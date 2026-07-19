//! Contract tests for protocol-neutral incremental SSE framing.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use futures_util::{StreamExt as _, stream};
use philo::{
    ByteStream, ErrorStage, LlmError, RetriableHint, SseConfig, SseDecoder, SseError, SseEvent,
    SseLimit, TransportError,
};
use proptest::prelude::*;

async fn decode_chunks(chunks: Vec<Bytes>, config: SseConfig) -> Vec<Result<SseEvent, SseError>> {
    let body: ByteStream = Box::pin(stream::iter(chunks.into_iter().map(Ok)));
    SseDecoder::with_config(body, config).collect().await
}

async fn decode_valid(chunks: Vec<Bytes>) -> Vec<SseEvent> {
    decode_chunks(chunks, SseConfig::default())
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn chunk_by_sizes(input: &[u8], sizes: &[usize]) -> Vec<Bytes> {
    let mut chunks = Vec::new();
    let mut offset = 0;
    for size in sizes {
        if offset == input.len() {
            break;
        }
        let end = offset.saturating_add((*size).max(1)).min(input.len());
        chunks.push(Bytes::copy_from_slice(&input[offset..end]));
        offset = end;
    }
    if offset < input.len() {
        chunks.push(Bytes::copy_from_slice(&input[offset..]));
    }
    chunks
}

fn property_config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 96;
    }
    config
}

#[tokio::test]
async fn every_byte_boundary_and_unicode_boundary_are_equivalent() {
    let input = b": heartbeat\r\nevent: message\r\nid: event-1\r\nretry: 2500\r\ndata: hello\r\ndata: \xE4\xB8\x96\xE7\x95\x8C\r\n\r\ndata: [DONE]\r\n\r\n";
    let baseline = decode_valid(vec![Bytes::from_static(input)]).await;

    for split in 0..=input.len() {
        let actual = decode_valid(vec![
            Bytes::copy_from_slice(&input[..split]),
            Bytes::copy_from_slice(&input[split..]),
        ])
        .await;
        assert_eq!(actual, baseline, "split at byte {split}");
    }

    let byte_chunks = input
        .iter()
        .map(|byte| Bytes::copy_from_slice(std::slice::from_ref(byte)))
        .collect();
    assert_eq!(decode_valid(byte_chunks).await, baseline);
    assert_eq!(baseline.len(), 2);
    assert_eq!(baseline[0].data(), "hello\n世界");
    assert_eq!(baseline[0].event_type(), Some("message"));
    assert_eq!(baseline[0].id(), Some("event-1"));
    assert_eq!(baseline[0].retry_millis(), Some(2500));
    assert_eq!(baseline[1].data(), "[DONE]");
    assert_eq!(baseline[1].id(), Some("event-1"));
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn random_chunking_matches_single_chunk(
        crlf in any::<bool>(),
        include_comment in any::<bool>(),
        data_lines in prop::collection::vec(
            prop_oneof![
                Just(String::new()),
                Just("alpha".to_owned()),
                Just("你好".to_owned()),
                Just("🙂".to_owned()),
            ],
            1..7,
        ),
        chunk_sizes in prop::collection::vec(1usize..24, 0..40),
    ) {
        let newline = if crlf { "\r\n" } else { "\n" };
        let mut source = String::new();
        if include_comment {
            source.push_str(": heartbeat");
            source.push_str(newline);
        }
        source.push_str("event: message");
        source.push_str(newline);
        for line in data_lines {
            source.push_str("data: ");
            source.push_str(&line);
            source.push_str(newline);
        }
        source.push_str(newline);
        source.push_str("data: [DONE]");
        source.push_str(newline);
        source.push_str(newline);
        let bytes = source.into_bytes();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (baseline, chunked) = runtime.block_on(async {
            let baseline = decode_valid(vec![Bytes::copy_from_slice(&bytes)]).await;
            let chunks = chunk_by_sizes(&bytes, &chunk_sizes);
            let chunked = decode_valid(chunks).await;
            (baseline, chunked)
        });
        prop_assert_eq!(chunked, baseline);
    }
}

#[tokio::test]
async fn empty_data_and_eof_dispatch_rules_are_explicit() {
    let events = decode_valid(vec![Bytes::from_static(
        b": comment only\n\ndata:\n\ndata: unterminated",
    )])
    .await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data(), "");
    assert_eq!(events[1].data(), "unterminated");

    let no_events = decode_valid(vec![Bytes::from_static(b": final comment")]).await;
    assert!(no_events.is_empty());

    let id_only = decode_valid(vec![Bytes::from_static(
        b"id: retained\n\ndata: next event\n\n",
    )])
    .await;
    assert_eq!(id_only[0].id(), Some("retained"));
}

#[tokio::test]
async fn invalid_utf8_and_resource_limits_are_typed_and_terminal() {
    let invalid = decode_chunks(
        vec![Bytes::from_static(b"data: \xff\n\n")],
        SseConfig::default(),
    )
    .await;
    assert!(matches!(
        invalid.as_slice(),
        [Err(SseError::InvalidUtf8 { line: 1 })]
    ));

    let line_limited = decode_chunks(
        vec![Bytes::from_static(b"data: long\n\n")],
        SseConfig::new(128, 4).unwrap(),
    )
    .await;
    assert!(matches!(
        line_limited.as_slice(),
        [Err(SseError::LimitExceeded {
            resource: SseLimit::LineBytes,
            limit: 4,
            observed: 5,
            ..
        })]
    ));

    let event_limited = decode_chunks(
        vec![Bytes::from_static(b"data: abc\n\n")],
        SseConfig::new(5, 64).unwrap(),
    )
    .await;
    assert!(matches!(
        event_limited.as_slice(),
        [Err(SseError::LimitExceeded {
            resource: SseLimit::EventBytes,
            limit: 5,
            observed: 6,
            ..
        })]
    ));

    let field_limited = decode_chunks(
        vec![Bytes::from_static(b"event: one\ndata: two\n\n")],
        SseConfig::new(128, 64)
            .unwrap()
            .with_max_fields_per_event(Some(1)),
    )
    .await;
    assert!(matches!(
        field_limited.as_slice(),
        [Err(SseError::LimitExceeded {
            resource: SseLimit::Fields,
            limit: 1,
            observed: 2,
            ..
        })]
    ));
}

#[tokio::test]
async fn upstream_error_classification_is_preserved() {
    let body: ByteStream = Box::pin(stream::iter([Err(LlmError::Transport(
        TransportError::new(ErrorStage::Body, RetriableHint::Maybe),
    ))]));
    let results: Vec<_> = SseDecoder::new(body).collect().await;
    assert!(matches!(
        results.as_slice(),
        [Err(SseError::Upstream(LlmError::Transport(error)))]
            if error.stage() == ErrorStage::Body
    ));

    let cancelled: ByteStream = Box::pin(stream::iter([Err(LlmError::Cancelled)]));
    let results: Vec<_> = SseDecoder::new(cancelled).collect().await;
    assert!(matches!(
        results.as_slice(),
        [Err(SseError::Upstream(LlmError::Cancelled))]
    ));
}

struct CountingStream {
    chunks: VecDeque<Result<Bytes, LlmError>>,
    polls: Arc<AtomicUsize>,
}

impl Stream for CountingStream {
    type Item = Result<Bytes, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(self.chunks.pop_front())
    }
}

#[tokio::test]
async fn slow_consumer_does_not_poll_past_buffered_event() {
    let polls = Arc::new(AtomicUsize::new(0));
    let body: ByteStream = Box::pin(CountingStream {
        chunks: VecDeque::from([
            Ok(Bytes::from_static(b"data: one\n\ndata: two\n\n")),
            Ok(Bytes::from_static(b"data: three\n\n")),
        ]),
        polls: Arc::clone(&polls),
    });
    let mut decoder = SseDecoder::new(body);

    assert_eq!(decoder.next().await.unwrap().unwrap().data(), "one");
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!(decoder.next().await.unwrap().unwrap().data(), "two");
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!(decoder.next().await.unwrap().unwrap().data(), "three");
    assert_eq!(polls.load(Ordering::SeqCst), 2);
}
