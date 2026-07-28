//! Shared protocol-private driver for SSE-backed response state machines.

use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

use crate::domain::AssistantEvent;
use crate::error::LlmError;
use crate::transport::{ByteStream, SseConfig, SseDecoder, SseEvent};

/// Protocol-local semantics consumed by the shared SSE polling mechanism.
pub(crate) trait SseEventMachine {
    fn accept(&mut self, event: &SseEvent) -> Result<Vec<AssistantEvent>, LlmError>;

    fn finish(&mut self) -> Result<Vec<AssistantEvent>, LlmError>;
}

/// Drives one SSE decoder and one protocol-local response state machine.
pub(crate) struct SseMachineStream<M> {
    source: SseDecoder,
    machine: M,
    pending: VecDeque<Result<AssistantEvent, LlmError>>,
    max_events_per_poll: usize,
    terminal: bool,
}

impl<M> SseMachineStream<M> {
    pub(crate) fn new(body: ByteStream, sse: SseConfig, machine: M) -> Self {
        Self {
            source: SseDecoder::with_config(body, sse),
            machine,
            pending: VecDeque::new(),
            max_events_per_poll: sse.max_events_per_poll(),
            terminal: false,
        }
    }
}

impl<M> fmt::Debug for SseMachineStream<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SseMachineStream")
            .field("machine", &std::any::type_name::<M>())
            .field("pending_events", &self.pending.len())
            .field("max_events_per_poll", &self.max_events_per_poll)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl<M> Stream for SseMachineStream<M>
where
    M: SseEventMachine + Unpin,
{
    type Item = Result<AssistantEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        if let Some(item) = stream.pending.pop_front() {
            return Poll::Ready(Some(item));
        }
        if stream.terminal {
            return Poll::Ready(None);
        }

        let mut events_processed = 0;
        loop {
            if events_processed >= stream.max_events_per_poll {
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            match Pin::new(&mut stream.source).poll_next(context) {
                Poll::Ready(Some(Ok(event))) => match stream.machine.accept(&event) {
                    Ok(events) => {
                        events_processed += 1;
                        stream.pending.extend(events.into_iter().map(Ok));
                        if let Some(item) = stream.pending.pop_front() {
                            return Poll::Ready(Some(item));
                        }
                    }
                    Err(error) => {
                        stream.terminal = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                },
                Poll::Ready(Some(Err(error))) => {
                    stream.terminal = true;
                    return Poll::Ready(Some(Err(error.into_llm_error())));
                }
                Poll::Ready(None) => {
                    stream.terminal = true;
                    match stream.machine.finish() {
                        Ok(events) => {
                            stream.pending.extend(events.into_iter().map(Ok));
                            return Poll::Ready(stream.pending.pop_front());
                        }
                        Err(error) => return Poll::Ready(Some(Err(error))),
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use futures_core::Stream;
    use futures_util::StreamExt as _;
    use futures_util::stream;
    use futures_util::task::{ArcWake, waker};

    use crate::domain::{AssistantEvent, FinishReason};
    use crate::error::{LlmError, ProtocolError};
    use crate::transport::{ByteStream, SseConfig, SseEvent};

    use super::{SseEventMachine, SseMachineStream};

    struct SyntheticMachine {
        accepts: Arc<AtomicUsize>,
        finishes: Arc<AtomicUsize>,
        events_per_accept: usize,
        terminal_events: usize,
        fail_on_accept: bool,
        debug_canary: String,
    }

    impl SyntheticMachine {
        fn new() -> Self {
            Self {
                accepts: Arc::new(AtomicUsize::new(0)),
                finishes: Arc::new(AtomicUsize::new(0)),
                events_per_accept: 0,
                terminal_events: 0,
                fail_on_accept: false,
                debug_canary: "machine-payload-canary".to_owned(),
            }
        }
    }

    impl SseEventMachine for SyntheticMachine {
        fn accept(&mut self, _event: &SseEvent) -> Result<Vec<AssistantEvent>, LlmError> {
            self.accepts.fetch_add(1, Ordering::SeqCst);
            if self.fail_on_accept {
                return Err(ProtocolError::new("synthetic machine failure").into());
            }
            Ok((0..self.events_per_accept).map(|_| done()).collect())
        }

        fn finish(&mut self) -> Result<Vec<AssistantEvent>, LlmError> {
            self.finishes.fetch_add(1, Ordering::SeqCst);
            Ok((0..self.terminal_events).map(|_| done()).collect())
        }
    }

    fn done() -> AssistantEvent {
        AssistantEvent::Done {
            finish_reason: FinishReason::Stop,
        }
    }

    fn body(input: impl Into<Bytes>) -> ByteStream {
        Box::pin(stream::iter([Ok(input.into())]))
    }

    fn config_with_event_budget(limit: usize) -> SseConfig {
        SseConfig::default()
            .with_poll_budget(1024 * 1024, 16, limit)
            .unwrap()
    }

    #[test]
    fn pending_events_are_drained_before_source_is_polled() {
        let mut machine = SyntheticMachine::new();
        machine.events_per_accept = 2;
        let accepts = Arc::clone(&machine.accepts);
        let mut stream = SseMachineStream::new(
            body("data: first\n\ndata: second\n\n"),
            SseConfig::default(),
            machine,
        );
        let task_waker = futures_util::task::noop_waker_ref();
        let mut context = Context::from_waker(task_waker);

        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut context),
            Poll::Ready(Some(Ok(AssistantEvent::Done { .. })))
        ));
        assert_eq!(accepts.load(Ordering::SeqCst), 1);
        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut context),
            Poll::Ready(Some(Ok(AssistantEvent::Done { .. })))
        ));
        assert_eq!(accepts.load(Ordering::SeqCst), 1);
    }

    struct WakeCounter(AtomicUsize);

    impl ArcWake for WakeCounter {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn fairness_budget_yields_and_wakes_without_losing_events() {
        let machine = SyntheticMachine::new();
        let accepts = Arc::clone(&machine.accepts);
        let mut stream = SseMachineStream::new(
            body("data: first\n\ndata: second\n\n"),
            config_with_event_budget(1),
            machine,
        );
        let wake_counter = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let task_waker = waker(Arc::clone(&wake_counter));
        let mut context = Context::from_waker(&task_waker);

        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut context),
            Poll::Pending
        ));
        assert_eq!(accepts.load(Ordering::SeqCst), 1);
        assert_eq!(wake_counter.0.load(Ordering::SeqCst), 1);
        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut context),
            Poll::Pending
        ));
        assert_eq!(accepts.load(Ordering::SeqCst), 2);
        assert_eq!(wake_counter.0.load(Ordering::SeqCst), 2);
        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut context),
            Poll::Ready(None)
        ));
    }

    #[tokio::test]
    async fn framing_error_is_emitted_once_then_stream_is_fused() {
        let machine = SyntheticMachine::new();
        let finishes = Arc::clone(&machine.finishes);
        let mut stream = SseMachineStream::new(
            body(Bytes::from_static(&[0xff, b'\n', b'\n'])),
            SseConfig::default(),
            machine,
        );

        assert!(matches!(
            stream.next().await,
            Some(Err(LlmError::Protocol(_)))
        ));
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
        assert_eq!(finishes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn machine_error_is_emitted_once_then_stream_is_fused() {
        let mut machine = SyntheticMachine::new();
        machine.fail_on_accept = true;
        let finishes = Arc::clone(&machine.finishes);
        let mut stream =
            SseMachineStream::new(body("data: fail\n\n"), SseConfig::default(), machine);

        assert!(matches!(
            stream.next().await,
            Some(Err(LlmError::Protocol(_)))
        ));
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
        assert_eq!(finishes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn eof_calls_finish_once_and_drains_all_terminal_events() {
        let mut machine = SyntheticMachine::new();
        machine.terminal_events = 2;
        let finishes = Arc::clone(&machine.finishes);
        let empty: ByteStream = Box::pin(stream::empty());
        let mut response = SseMachineStream::new(empty, SseConfig::default(), machine);

        assert!(matches!(
            response.next().await,
            Some(Ok(AssistantEvent::Done { .. }))
        ));
        assert!(matches!(
            response.next().await,
            Some(Ok(AssistantEvent::Done { .. }))
        ));
        assert!(response.next().await.is_none());
        assert!(response.next().await.is_none());
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
    }

    struct DropProbe {
        polls: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl Stream for DropProbe {
        type Item = Result<Bytes, LlmError>;

        fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Poll::Pending
        }
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn drop_does_not_poll_and_debug_does_not_render_machine_state() {
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let body: ByteStream = Box::pin(DropProbe {
            polls: Arc::clone(&polls),
            drops: Arc::clone(&drops),
        });
        let machine = SyntheticMachine::new();
        assert_eq!(machine.debug_canary, "machine-payload-canary");
        let response = SseMachineStream::new(body, SseConfig::default(), machine);

        let debug = format!("{response:?}");
        assert!(debug.contains("pending_events"));
        assert!(debug.contains("max_events_per_poll"));
        assert!(debug.contains("terminal"));
        assert!(!debug.contains("machine-payload-canary"));
        drop(response);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
