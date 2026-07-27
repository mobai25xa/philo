//! Protocol-specific request and response translation.

use std::fmt;
use std::pin::Pin;

use bytes::Bytes;
use futures_core::Stream;
use http::{HeaderName, Method, StatusCode};

use crate::domain::AssistantEvent;
use crate::domain::{LocalRequestId, ModelRef, ProviderRequestId, ResponseFormat, SourceIdentity};
use crate::error::LlmError;
use crate::plan::{
    CallExecutionIntent, ProtocolKind, ResolvedCallPlan, ResolvedTarget, ResponseLimits,
};
use crate::provider::HeaderOperation;
use crate::provider::{AnthropicMessagesContract, OpenAiChatContract};
use crate::transport::SseConfig;

pub(crate) type EventStream =
    Pin<Box<dyn Stream<Item = Result<AssistantEvent, LlmError>> + Send + 'static>>;

pub(crate) mod anthropic_messages;
pub(crate) mod openai_chat;
mod response;

pub(crate) use response::ResponseSession;

/// Concrete protocol dispatch selected from the runtime-validated protocol kind.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ProtocolDispatch {
    OpenAiChat(openai_chat::OpenAiChatDriver),
    AnthropicMessages(anthropic_messages::AnthropicMessagesDriver),
}

impl ProtocolDispatch {
    pub(crate) fn for_kind(kind: ProtocolKind) -> Self {
        match kind {
            ProtocolKind::OpenAiChatCompletions => Self::OpenAiChat(openai_chat::OpenAiChatDriver),
            ProtocolKind::AnthropicMessages => {
                Self::AnthropicMessages(anthropic_messages::AnthropicMessagesDriver)
            }
        }
    }

    pub(crate) fn prepare(self, plan: &ResolvedCallPlan) -> Result<PreparedCall, LlmError> {
        match self {
            Self::OpenAiChat(driver) => driver.prepare(plan),
            Self::AnthropicMessages(driver) => driver.prepare(plan),
        }
    }
}

/// Owned protocol output that can be executed without reading the source request.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct PreparedCall {
    pub(crate) target: ResolvedTarget,
    pub(crate) request: ProtocolRequestParts,
    pub(crate) response: ResponsePlan,
    pub(crate) facts: RequestFacts,
    pub(crate) execution: CallExecutionIntent,
}

impl fmt::Debug for PreparedCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedCall")
            .field("target", &self.target)
            .field("request", &self.request)
            .field("response", &self.response)
            .field("facts", &self.facts)
            .field("execution", &self.execution)
            .finish()
    }
}

/// Protocol-owned HTTP method, operation, header intents, and serialized body.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct ProtocolRequestParts {
    pub(crate) method: Method,
    pub(crate) operation: ProtocolOperation,
    pub(crate) protocol_headers: Vec<HeaderOperation>,
    pub(crate) body: Bytes,
}

impl fmt::Debug for ProtocolRequestParts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtocolRequestParts")
            .field("method", &self.method)
            .field("operation", &self.operation)
            .field("protocol_header_count", &self.protocol_headers.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Typed endpoint operation selected by a protocol driver.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtocolOperation {
    /// OpenAI-compatible `POST /chat/completions`.
    ChatCompletions,
    /// Anthropic `POST /v1/messages`.
    Messages,
}

/// Low-sensitivity request facts used by dynamic header policy.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RequestFacts {
    pub(crate) contains_tools: bool,
    pub(crate) contains_images: bool,
    pub(crate) reasoning_enabled: bool,
    pub(crate) response_format: ResponseFormatKind,
    pub(crate) max_output_tokens_source: MaxOutputTokensSource,
}

/// Value-free source of the resolved output-token request field.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MaxOutputTokensSource {
    Request,
    ModelDefault,
    Omitted,
}

/// Value-free response-format classification.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResponseFormatKind {
    Text,
    JsonObject,
    JsonSchema,
}

impl From<&ResponseFormat> for ResponseFormatKind {
    fn from(value: &ResponseFormat) -> Self {
        match value {
            ResponseFormat::Text => Self::Text,
            ResponseFormat::JsonObject => Self::JsonObject,
            ResponseFormat::JsonSchema(_) => Self::JsonSchema,
        }
    }
}

/// HTTP and protocol requirements needed to open a response session.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ResponsePlan {
    pub(crate) http: HttpResponseRequirements,
    pub(crate) protocol: ProtocolResponsePlan,
}

/// Protocol-specific response plan selected by the concrete driver.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum ProtocolResponsePlan {
    OpenAiChat(OpenAiChatResponsePlan),
    AnthropicMessages(AnthropicMessagesResponsePlan),
}

/// HTTP response properties checked before protocol decoding.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HttpResponseRequirements {
    pub(crate) content_type: ExpectedContentType,
    pub(crate) max_error_body_bytes: usize,
}

/// Expected successful response media family.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpectedContentType {
    EventStream,
}

/// Inputs required to create an `OpenAI` Chat response state machine.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct OpenAiChatResponsePlan {
    pub(crate) model: ModelRef,
    pub(crate) response_format: ResponseFormat,
    pub(crate) contract: OpenAiChatContract,
    pub(crate) limits: ResponseLimits,
    pub(crate) sse: SseConfig,
}

impl fmt::Debug for OpenAiChatResponsePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiChatResponsePlan")
            .field("model", &self.model)
            .field(
                "response_format",
                &ResponseFormatKind::from(&self.response_format),
            )
            .field("contract", &"openai-chat")
            .field("limits", &self.limits)
            .field("sse", &self.sse)
            .finish()
    }
}

/// Inputs required to create an Anthropic Messages response state machine.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct AnthropicMessagesResponsePlan {
    pub(crate) source: SourceIdentity,
    pub(crate) response_format: ResponseFormat,
    pub(crate) contract: AnthropicMessagesContract,
    pub(crate) limits: ResponseLimits,
    pub(crate) sse: SseConfig,
}

impl fmt::Debug for AnthropicMessagesResponsePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicMessagesResponsePlan")
            .field("source", &self.source)
            .field(
                "response_format",
                &ResponseFormatKind::from(&self.response_format),
            )
            .field("contract", &"anthropic-messages")
            .field("limits", &self.limits)
            .field("sse", &self.sse)
            .finish()
    }
}

/// Value-free response metadata admitted into protocol state.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResponseMeta {
    pub(crate) local_request_id: LocalRequestId,
    pub(crate) provider_request_id: Option<ProviderRequestId>,
    pub(crate) status: StatusCode,
    pub(crate) header_names: Vec<HeaderName>,
    pub(crate) retry_after: Option<std::time::Duration>,
    pub(crate) rate_limit: crate::provider::RateLimitObservation,
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::{HeaderValue, Method, header};

    use super::{ProtocolDispatch, ProtocolOperation, ProtocolRequestParts};
    use crate::plan::ProtocolKind;
    use crate::provider::HeaderOperation;

    const CANARY: &str = "protocol-contract-canary";

    #[test]
    fn protocol_request_debug_reports_shape_without_values_or_body() {
        let request = ProtocolRequestParts {
            method: Method::POST,
            operation: ProtocolOperation::ChatCompletions,
            protocol_headers: vec![HeaderOperation::set(
                header::HeaderName::from_static("x-contract-canary"),
                HeaderValue::from_static(CANARY),
            )],
            body: Bytes::from_static(b"protocol-contract-canary"),
        };
        let debug = format!("{request:?}");
        assert!(debug.contains("body_bytes"));
        assert!(!debug.contains(CANARY));
    }

    #[test]
    fn dispatch_selects_each_validated_protocol_kind() {
        assert!(matches!(
            ProtocolDispatch::for_kind(ProtocolKind::OpenAiChatCompletions),
            ProtocolDispatch::OpenAiChat(_)
        ));
        assert!(matches!(
            ProtocolDispatch::for_kind(ProtocolKind::AnthropicMessages),
            ProtocolDispatch::AnthropicMessages(_)
        ));
    }
}
