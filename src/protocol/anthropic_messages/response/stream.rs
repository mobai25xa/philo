use super::machine::MessagesStateMachine;
use crate::domain::{LocalRequestId, ProviderRequestId, ResponseFormat, SourceIdentity};
use crate::plan::ResponseLimits;
use crate::protocol::response_stream::{SseEventMachine, SseMachineStream};
use crate::provider::AnthropicMessagesContract;
use crate::transport::{ByteStream, SseConfig, SseEvent};

#[derive(Clone, Debug)]
pub(crate) struct AnthropicMessagesStreamContext {
    pub(super) local_request_id: LocalRequestId,
    pub(super) provider_request_id: Option<ProviderRequestId>,
    pub(super) source: SourceIdentity,
}

impl AnthropicMessagesStreamContext {
    pub(crate) fn new(
        local_request_id: LocalRequestId,
        provider_request_id: Option<ProviderRequestId>,
        source: SourceIdentity,
    ) -> Self {
        Self {
            local_request_id,
            provider_request_id,
            source,
        }
    }
}

pub(crate) fn decode_anthropic_messages_stream(
    body: ByteStream,
    context: AnthropicMessagesStreamContext,
    response_format: ResponseFormat,
    sse: SseConfig,
    limits: ResponseLimits,
    contract: AnthropicMessagesContract,
) -> AnthropicMessagesEventStream {
    SseMachineStream::new(
        body,
        sse,
        MessagesStateMachine::new(context, response_format, limits, contract.usage),
    )
}

/// Anthropic-specific machine carried by the shared SSE stream driver.
pub(crate) type AnthropicMessagesEventStream = SseMachineStream<MessagesStateMachine>;

impl SseEventMachine for MessagesStateMachine {
    fn accept(
        &mut self,
        event: &SseEvent,
    ) -> Result<Vec<crate::domain::AssistantEvent>, crate::error::LlmError> {
        MessagesStateMachine::accept(self, event)
    }

    fn finish(&mut self) -> Result<Vec<crate::domain::AssistantEvent>, crate::error::LlmError> {
        MessagesStateMachine::finish(self)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{StreamExt as _, stream};

    use crate::domain::{
        AssistantEvent, LocalRequestId, ModelId, ProtocolId, ProviderId, ProviderRequestId,
        ResourceLimits, ResponseFormat, SourceIdentity, StructuredSchema, TokenCount, ToolSchema,
    };
    use crate::error::LlmError;
    use crate::plan::ResponseLimits;
    use crate::provider::AnthropicUsageCompat;
    use crate::transport::{ByteStream, SseConfig};

    use super::{AnthropicMessagesStreamContext, decode_anthropic_messages_stream};

    const TEXT: &[u8] =
        include_bytes!("../../../../tests/fixtures/phase-5/anthropic-messages/stream/text.sse");
    const TOOL: &[u8] =
        include_bytes!("../../../../tests/fixtures/phase-5/anthropic-messages/stream/tool-use.sse");
    const THINKING: &[u8] = include_bytes!(
        "../../../../tests/fixtures/phase-5/anthropic-messages/stream/thinking-signature.sse"
    );
    const PING_UNKNOWN: &[u8] = include_bytes!(
        "../../../../tests/fixtures/phase-5/anthropic-messages/stream/ping-unknown.sse"
    );
    const ERROR: &[u8] =
        include_bytes!("../../../../tests/fixtures/phase-5/anthropic-messages/stream/error.sse");
    const TRUNCATED: &[u8] = include_bytes!(
        "../../../../tests/fixtures/phase-5/anthropic-messages/stream/truncated.sse"
    );
    const USAGE_FINISH: &[u8] = include_bytes!(
        "../../../../tests/fixtures/phase-5/anthropic-messages/stream/usage-finish.sse"
    );
    const REDACTED: &[u8] = include_bytes!(
        "../../../../tests/fixtures/phase-5/anthropic-messages/stream/redacted-thinking.sse"
    );

    fn body(chunks: Vec<Bytes>) -> ByteStream {
        Box::pin(stream::iter(chunks.into_iter().map(Ok)))
    }

    async fn decode(chunks: Vec<Bytes>) -> Vec<Result<AssistantEvent, LlmError>> {
        decode_with_limits(chunks, ResponseLimits::from(ResourceLimits::official())).await
    }

    async fn decode_with_limits(
        chunks: Vec<Bytes>,
        limits: ResponseLimits,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        decode_with_format_and_usage_compat(
            chunks,
            ResponseFormat::Text,
            limits,
            AnthropicUsageCompat::StrictStableFields,
        )
        .await
    }

    async fn decode_with_usage_compat(
        chunks: Vec<Bytes>,
        limits: ResponseLimits,
        usage_compat: AnthropicUsageCompat,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        decode_with_format_and_usage_compat(chunks, ResponseFormat::Text, limits, usage_compat)
            .await
    }

    async fn decode_with_format_and_usage_compat(
        chunks: Vec<Bytes>,
        response_format: ResponseFormat,
        limits: ResponseLimits,
        usage_compat: AnthropicUsageCompat,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        decode_anthropic_messages_stream(
            body(chunks),
            AnthropicMessagesStreamContext::new(
                LocalRequestId::new("anthropic-stream-test").unwrap(),
                Some(ProviderRequestId::new("req_stream_test").unwrap()),
                SourceIdentity::new(
                    ProviderId::new("test-only").unwrap(),
                    ModelId::new("claude-test").unwrap(),
                    ProtocolId::new("anthropic-messages").unwrap(),
                ),
            ),
            response_format,
            SseConfig::default(),
            limits,
            crate::provider::AnthropicMessagesContract::strict_official()
                .with_usage_compat(usage_compat),
        )
        .collect()
        .await
    }

    fn structured_text(text: &str) -> Bytes {
        Bytes::from(format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_structured\",\"model\":\"claude-test\",\"usage\":{{}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":{}}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
            serde_json::to_string(text).unwrap()
        ))
    }

    async fn decode_one(input: &'static [u8]) -> Vec<Result<AssistantEvent, LlmError>> {
        decode(vec![Bytes::from_static(input)]).await
    }

    #[tokio::test]
    async fn valid_text_lifecycle_emits_start_text_usage_done() {
        let events = decode_one(TEXT)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(events.first(), Some(AssistantEvent::Start { .. })));
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::TextDelta { delta, .. } if delta == "Hello"
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantEvent::DetailedUsage(_)))
        );
        let usage = events.iter().find_map(|event| match event {
            AssistantEvent::DetailedUsage(usage) => Some(*usage),
            _ => None,
        });
        let usage = usage.expect("terminal usage snapshot");
        assert_eq!(usage.cached_input_tokens(), TokenCount::Known(0));
        assert_eq!(usage.cache_write_tokens(), TokenCount::Known(0));
        assert_eq!(usage.reasoning_tokens(), TokenCount::Unknown);
        assert_eq!(usage.total_tokens(), TokenCount::Unknown);
        assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantEvent::Done { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn invalid_structured_json_fails_before_done() {
        let results = decode_with_format_and_usage_compat(
            vec![structured_text("not-json")],
            ResponseFormat::JsonObject,
            ResponseLimits::from(ResourceLimits::official()),
            AnthropicUsageCompat::StrictStableFields,
        )
        .await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::StructuredOutput(error)))
                if error.reason() == crate::error::StructuredOutputFailure::InvalidJson
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn structured_schema_violation_fails_before_done() {
        let format = ResponseFormat::JsonSchema(
            StructuredSchema::new(
                "answer",
                None,
                ToolSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                    "additionalProperties": false
                }))
                .unwrap(),
                true,
            )
            .unwrap(),
        );
        let results = decode_with_format_and_usage_compat(
            vec![structured_text("{\"ok\":\"wrong\"}")],
            format,
            ResponseLimits::from(ResourceLimits::official()),
            AnthropicUsageCompat::StrictStableFields,
        )
        .await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::StructuredOutput(error)))
                if error.reason() == crate::error::StructuredOutputFailure::SchemaViolation
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn structured_output_limit_fails_during_accumulation_before_done() {
        let mut limits = ResponseLimits::from(ResourceLimits::official());
        limits.max_structured_output_bytes = 2;
        let results = decode_with_format_and_usage_compat(
            vec![structured_text("{\"ok\":true}")],
            ResponseFormat::JsonObject,
            limits,
            AnthropicUsageCompat::StrictStableFields,
        )
        .await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn tool_end_requires_complete_valid_json() {
        let events = decode_one(TOOL)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::ToolCallEnd { call, .. }
                if call.arguments().value()["city"] == "Paris"
        )));
        assert!(matches!(
            events.last(),
            Some(AssistantEvent::Done {
                finish_reason: crate::domain::FinishReason::ToolCalls
            })
        ));

        let invalid = String::from_utf8(TOOL.to_vec())
            .unwrap()
            .replace("\\\"Paris\\\"}", "\\\"Paris\\\"");
        let results = decode(vec![Bytes::from(invalid)]).await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(!results.iter().any(|item| matches!(
            item,
            Ok(AssistantEvent::ToolCallEnd { .. } | AssistantEvent::Done { .. })
        )));
    }

    #[tokio::test]
    async fn thinking_signature_is_never_answer_text() {
        let events = decode_one(THINKING)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::ThinkingDelta { delta, .. } if delta == "Summary"
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            AssistantEvent::TextDelta { delta, .. } if delta.contains("opaque_test_signature")
        )));
        let opaque = events.iter().find_map(|event| match event {
            AssistantEvent::ThinkingOpaque { opaque, .. } => Some(opaque),
            _ => None,
        });
        let opaque = opaque.expect("signature must be retained as opaque state");
        assert!(!opaque.is_redacted());
        assert_eq!(opaque.source().protocol().as_str(), "anthropic-messages");
        assert!(opaque.source().generation_id().is_some());
        assert!(!format!("{opaque:?}").contains("opaque_test_signature"));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantEvent::DetailedUsage(_)))
        );
    }

    #[tokio::test]
    async fn redacted_thinking_is_opaque_and_never_answer_text() {
        let events = decode_one(REDACTED)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let opaque = events.iter().find_map(|event| match event {
            AssistantEvent::ThinkingOpaque { opaque, .. } => Some(opaque),
            _ => None,
        });
        let opaque = opaque.expect("redacted block must become opaque thinking");
        assert!(opaque.is_redacted());
        assert!(!events.iter().any(|event| matches!(
            event,
            AssistantEvent::TextDelta { delta, .. } if delta.contains("opaque_redacted_canary")
        )));
        assert!(!format!("{events:?}").contains("opaque_redacted_canary"));
        let collected = crate::domain::collect_assistant_message(stream::iter(
            events.clone().into_iter().map(Ok),
        ))
        .await
        .unwrap();
        let thinking = collected.content().iter().find_map(|part| match part {
            crate::domain::ContentPart::Thinking(thinking) => Some(thinking),
            _ => None,
        });
        assert!(thinking.and_then(|thinking| thinking.opaque()).is_some());
    }

    #[tokio::test]
    async fn ping_and_unknown_event_emit_no_domain_content() {
        let events = decode_one(PING_UNKNOWN)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantEvent::TextDelta { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn stream_error_terminates_without_done_or_message_leak() {
        let results = decode_one(ERROR).await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::Protocol(error)))
                if error.provider_code() == Some("overloaded_error")
                    && error.request_id().is_some()
                    && error.retriable() == crate::error::RetriableHint::Maybe
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
        let debug = format!("{:?}", results.last().unwrap());
        assert!(!debug.contains("stream-error-body-canary"));
    }

    #[tokio::test]
    async fn usage_snapshots_are_cumulative_and_preserve_unknown_zero() {
        let events = decode_one(USAGE_FINISH)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let details = events.iter().find_map(|event| match event {
            AssistantEvent::DetailedUsage(details) => Some(*details),
            _ => None,
        });
        let details = details.expect("terminal usage snapshot");
        assert_eq!(details.input_tokens(), TokenCount::Known(15));
        assert_eq!(details.output_tokens(), TokenCount::Known(5));
        assert_eq!(details.total_tokens(), TokenCount::Unknown);
        assert_eq!(details.cached_input_tokens(), TokenCount::Known(3));
        assert_eq!(details.cache_write_tokens(), TokenCount::Known(2));
        assert_eq!(details.reasoning_tokens(), TokenCount::Known(2));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantEvent::DetailedUsage(_)))
                .count(),
            1
        );

        let regression = String::from_utf8(USAGE_FINISH.to_vec())
            .unwrap()
            .replace("\"output_tokens\":5", "\"output_tokens\":1");
        assert!(matches!(
            decode(vec![Bytes::from(regression)]).await.last(),
            Some(Err(LlmError::Protocol(_)))
        ));
    }

    #[tokio::test]
    async fn compatible_usage_may_increase_stable_fields_but_never_decrease() {
        let evolving = String::from_utf8(USAGE_FINISH.to_vec()).unwrap().replace(
            "\"usage\":{\"output_tokens\":5,\"thinking_tokens\":2}",
            "\"usage\":{\"input_tokens\":12,\"output_tokens\":5,\"cache_creation_input_tokens\":4,\"cache_read_input_tokens\":3,\"thinking_tokens\":2}",
        );
        assert!(matches!(
            decode(vec![Bytes::from(evolving.clone())]).await.last(),
            Some(Err(LlmError::Protocol(_)))
        ));

        let events = decode_with_usage_compat(
            vec![Bytes::from(evolving)],
            ResponseLimits::from(ResourceLimits::official()),
            AnthropicUsageCompat::AllowMonotonicStableFields,
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let details = events.iter().find_map(|event| match event {
            AssistantEvent::DetailedUsage(details) => Some(*details),
            _ => None,
        });
        let details = details.expect("terminal compatible usage snapshot");
        assert_eq!(details.input_tokens(), TokenCount::Known(19));

        let decreasing = String::from_utf8(USAGE_FINISH.to_vec()).unwrap().replace(
            "\"usage\":{\"output_tokens\":5,\"thinking_tokens\":2}",
            "\"usage\":{\"input_tokens\":9,\"output_tokens\":5,\"thinking_tokens\":2}",
        );
        assert!(matches!(
            decode_with_usage_compat(
                vec![Bytes::from(decreasing)],
                ResponseLimits::from(ResourceLimits::official()),
                AnthropicUsageCompat::AllowMonotonicStableFields,
            )
            .await
            .last(),
            Some(Err(LlmError::Protocol(_)))
        ));
    }

    #[tokio::test]
    async fn finish_reason_mapping_is_known_or_fail_closed_with_raw() {
        let length = String::from_utf8(TEXT.to_vec())
            .unwrap()
            .replace("\"end_turn\"", "\"max_tokens\"");
        let events = decode(vec![Bytes::from(length)])
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            events.last(),
            Some(AssistantEvent::Done {
                finish_reason: crate::domain::FinishReason::Length
            })
        ));

        let unknown = String::from_utf8(TEXT.to_vec())
            .unwrap()
            .replace("\"end_turn\"", "\"future_reason\"");
        assert!(matches!(
            decode(vec![Bytes::from(unknown)]).await.last(),
            Some(Err(LlmError::UnknownFinishReason(error))) if error.raw() == "future_reason"
        ));

        let paused = String::from_utf8(TEXT.to_vec())
            .unwrap()
            .replace("\"end_turn\"", "\"pause_turn\"");
        assert!(matches!(
            decode(vec![Bytes::from(paused)]).await.last(),
            Some(Err(LlmError::UnsupportedResponseSemantics(error))) if error.raw() == "pause_turn"
        ));
    }

    #[tokio::test]
    async fn eof_before_message_stop_is_truncated() {
        let results = decode_one(TRUNCATED).await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::TruncatedStream(_)))
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn invalid_block_transitions_fail_closed() {
        let without_start = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"model\":\"claude-test\",\"usage\":{}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"bad\"}}\n\n";
        assert!(matches!(
            decode(vec![Bytes::from_static(without_start)]).await.last(),
            Some(Err(LlmError::Protocol(_)))
        ));

        let duplicate_start = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\",\"model\":\"claude-test\",\"usage\":{}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
        assert!(matches!(
            decode(vec![Bytes::from_static(duplicate_start)])
                .await
                .last(),
            Some(Err(LlmError::Protocol(_)))
        ));

        let duplicate_stop = String::from_utf8(TEXT.to_vec()).unwrap().replace(
            "event: message_delta",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta",
        );
        let results = decode(vec![Bytes::from(duplicate_stop)]).await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn event_name_and_json_type_must_match() {
        let mismatch = b"event: ping\ndata: {\"type\":\"message_stop\"}\n\n";
        assert!(matches!(
            decode(vec![Bytes::from_static(mismatch)]).await.as_slice(),
            [Err(LlmError::Protocol(_))]
        ));
    }

    #[tokio::test]
    async fn event_after_message_stop_prevents_done() {
        let mut input = TEXT.to_vec();
        input.extend_from_slice(b"event: ping\ndata: {\"type\":\"ping\"}\n\n");
        let results = decode(vec![Bytes::from(input)]).await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn byte_fragmentation_preserves_event_sequence() {
        let expected = decode_one(TEXT)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for chunk_size in 1..=31 {
            let chunks = TEXT
                .chunks(chunk_size)
                .map(Bytes::copy_from_slice)
                .collect();
            let actual = decode(chunks)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(actual, expected, "chunk size {chunk_size}");
        }
    }

    #[tokio::test]
    async fn opening_two_streams_creates_fresh_response_state() {
        let first = decode_one(TEXT).await;
        let second = decode_one(TEXT).await;
        assert_eq!(
            first.into_iter().collect::<Result<Vec<_>, _>>().unwrap(),
            second.into_iter().collect::<Result<Vec<_>, _>>().unwrap()
        );
    }

    #[tokio::test]
    async fn decoder_accumulators_respect_resolved_limits() {
        let mut limits = ResponseLimits::from(ResourceLimits::official());
        limits.max_tool_arguments_bytes = 4;
        limits.max_all_tool_arguments_bytes = 4;
        let results = decode_with_limits(vec![Bytes::from_static(TOOL)], limits).await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(!results.iter().any(|item| matches!(
            item,
            Ok(AssistantEvent::ToolCallEnd { .. } | AssistantEvent::Done { .. })
        )));

        let mut limits = ResponseLimits::from(ResourceLimits::official());
        limits.max_structured_output_bytes = 4;
        let results = decode_with_limits(vec![Bytes::from_static(THINKING)], limits).await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }
}
