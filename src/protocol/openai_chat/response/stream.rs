use super::machine::ChatStateMachine;
use crate::domain::{LocalRequestId, ModelRef, ProviderRequestId, ResponseFormat};
use crate::plan::ResponseLimits;
use crate::protocol::response_stream::{SseEventMachine, SseMachineStream};
use crate::provider::ResponseCompat;
use crate::transport::{ByteStream, SseConfig, SseEvent};

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
    SseMachineStream::new(
        body,
        sse,
        ChatStateMachine::new_with_format(context, response_format, limits, compat),
    )
}

/// OpenAI-specific machine carried by the shared SSE stream driver.
pub(crate) type OpenAiChatEventStream = SseMachineStream<ChatStateMachine>;

impl SseEventMachine for ChatStateMachine {
    fn accept(
        &mut self,
        event: &SseEvent,
    ) -> Result<Vec<crate::domain::AssistantEvent>, crate::error::LlmError> {
        ChatStateMachine::accept(self, event)
    }

    fn finish(&mut self) -> Result<Vec<crate::domain::AssistantEvent>, crate::error::LlmError> {
        ChatStateMachine::finish(self)
    }
}
