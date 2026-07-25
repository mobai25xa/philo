use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

use super::machine::ChatStateMachine;
use crate::domain::{AssistantEvent, LocalRequestId, ModelRef, ProviderRequestId, ResponseFormat};
use crate::error::LlmError;
use crate::provider::ResponseCompat;
use crate::provider::call_policy::ResponseLimits;
use crate::transport::{ByteStream, SseConfig, SseDecoder};

/// Stable request context supplied by the client orchestration layer.
#[derive(Clone, Debug)]
pub(crate) struct OpenAiChatStreamContext {
    pub(super) local_request_id: LocalRequestId,
    pub(super) provider_request_id: Option<ProviderRequestId>,
    _model: ModelRef,
}

impl OpenAiChatStreamContext {
    pub(crate) fn new(
        local_request_id: LocalRequestId,
        provider_request_id: Option<ProviderRequestId>,
        model: ModelRef,
    ) -> Self {
        Self {
            local_request_id,
            provider_request_id,
            _model: model,
        }
    }
}

/// Converts response bytes using only the policy captured by planning.
#[allow(dead_code)]
pub(crate) fn decode_openai_chat_stream_with_plan(
    body: ByteStream,
    context: OpenAiChatStreamContext,
    response_format: ResponseFormat,
    sse: SseConfig,
    limits: ResponseLimits,
) -> OpenAiChatEventStream {
    decode_openai_chat_stream_with_policy(
        body,
        context,
        response_format,
        sse,
        limits,
        ResponseCompat::default(),
    )
}

/// Converts response bytes using the typed compatibility policy captured by planning.
pub(crate) fn decode_openai_chat_stream_with_policy(
    body: ByteStream,
    context: OpenAiChatStreamContext,
    response_format: ResponseFormat,
    sse: SseConfig,
    limits: ResponseLimits,
    compat: ResponseCompat,
) -> OpenAiChatEventStream {
    let max_events_per_poll = sse.max_events_per_poll();
    OpenAiChatEventStream {
        source: SseDecoder::with_config(body, sse),
        machine: ChatStateMachine::new_with_format(context, response_format, limits, compat),
        pending: VecDeque::new(),
        max_events_per_poll,
        terminal: false,
    }
}

/// Stream adapter joining the protocol-neutral SSE decoder to Chat semantics.
pub(crate) struct OpenAiChatEventStream {
    source: SseDecoder,
    machine: ChatStateMachine,
    pending: VecDeque<Result<AssistantEvent, LlmError>>,
    max_events_per_poll: usize,
    terminal: bool,
}

impl fmt::Debug for OpenAiChatEventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiChatEventStream")
            .field("machine", &self.machine)
            .field("pending_events", &self.pending.len())
            .field("max_events_per_poll", &self.max_events_per_poll)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl Stream for OpenAiChatEventStream {
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
