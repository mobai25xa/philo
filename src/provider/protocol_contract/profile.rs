//! Resolved `OpenAI` Chat compatibility contract and field provenance.
//!
//! This is the *resolved* form. The core does not merge sparse configuration
//! layers. It accepts one immutable contract when the provider definition is
//! built and carries that contract unchanged into every request.

use std::collections::BTreeMap;

use crate::domain::{DialectPolicy, PolicySource};

use super::{FinishReasonCompat, ToolArgumentsCompat, UsageCompat};
use super::{
    HistoryCompat, MaxOutputTokensWireFormat, ModelBodyWireFormat, RequestCompat, ResponseCompat,
};

/// Stable names for compatibility leaves.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CompatField {
    /// Request model field presence.
    RequestModelBody,
    /// Maximum output token request field.
    RequestMaxOutputTokens,
    /// Request tool choice.
    RequestToolChoice,
    /// Request thinking.
    RequestThinking,
    /// Request image.
    RequestImage,
    /// Request stream usage.
    RequestStreamUsage,
    /// Request structured output.
    RequestStructuredOutput,
    /// Response finish reason.
    ResponseFinishReason,
    /// Response tool arguments.
    ResponseToolArguments,
    /// Response usage.
    ResponseUsage,
    /// Response inline error.
    ResponseInlineError,
    /// History missing tool result.
    HistoryMissingToolResult,
    /// History unsupported content.
    HistoryUnsupportedContent,
    /// History thinking replay.
    HistoryThinkingReplay,
    /// History tool-result name.
    HistoryToolResultName,
    /// History tool-call id.
    HistoryToolCallId,
}

/// Complete, immutable compatibility policy compiled before request encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatProfile {
    request: RequestCompat,
    response: ResponseCompat,
    history: HistoryCompat,
    provenance: BTreeMap<CompatField, PolicySource>,
}

impl CompatProfile {
    /// Creates the `OpenAI` Chat protocol-default profile.
    #[must_use]
    pub fn openai_chat_default() -> Self {
        let provenance = CompatField::ALL
            .into_iter()
            .map(|field| (field, PolicySource::ProtocolDefault))
            .collect();
        Self {
            request: RequestCompat::openai_chat_default(),
            response: ResponseCompat::openai_chat_default(),
            history: HistoryCompat::openai_chat_default(),
            provenance,
        }
    }

    /// Returns request strategies.
    #[must_use]
    pub const fn request(&self) -> &RequestCompat {
        &self.request
    }

    /// Returns response strategies.
    #[must_use]
    pub const fn response(&self) -> &ResponseCompat {
        &self.response
    }

    /// Returns history strategies.
    #[must_use]
    pub const fn history(&self) -> &HistoryCompat {
        &self.history
    }

    /// Returns the source of one resolved leaf.
    #[must_use]
    pub fn source(&self, field: CompatField) -> PolicySource {
        self.provenance[&field]
    }

    /// Reconstructs the P2 dialect view used by domain history normalization.
    #[must_use]
    pub const fn dialect_policy(&self) -> DialectPolicy {
        DialectPolicy {
            source: PolicySource::ProtocolDefault,
            tool_choice: self.request.tool_choice,
            tool_result_name: self.history.tool_result_name,
            tool_call_id: self.history.tool_call_id,
            thinking: self.request.thinking,
            image: self.request.image,
            stream_usage: self.request.stream_usage,
            structured_output: self.request.structured_output,
        }
    }

    /// Returns mutable access to the resolved strategies.
    ///
    /// This is the seam a layered resolver writes through: apply the winning
    /// leaf, then record where it came from with [`Self::record_source`]. The
    /// core itself never layers; it only carries the result.
    pub fn parts_mut(&mut self) -> (&mut RequestCompat, &mut ResponseCompat, &mut HistoryCompat) {
        (&mut self.request, &mut self.response, &mut self.history)
    }

    /// Records the declaration layer responsible for one resolved leaf.
    pub fn record_source(&mut self, field: CompatField, source: PolicySource) {
        self.provenance.insert(field, source);
    }

    /// Sets whether the request body carries the resolved wire model.
    #[must_use]
    pub fn with_model_body(mut self, value: ModelBodyWireFormat, source: PolicySource) -> Self {
        self.request.model_body = value;
        self.record_source(CompatField::RequestModelBody, source);
        self
    }

    /// Sets the maximum output token wire format.
    #[must_use]
    pub fn with_max_output_tokens(
        mut self,
        value: MaxOutputTokensWireFormat,
        source: PolicySource,
    ) -> Self {
        self.request.max_output_tokens = value;
        self.record_source(CompatField::RequestMaxOutputTokens, source);
        self
    }

    /// Sets streamed finish-reason handling.
    #[must_use]
    pub fn with_finish_reason(mut self, value: FinishReasonCompat, source: PolicySource) -> Self {
        self.response.finish_reason = value;
        self.record_source(CompatField::ResponseFinishReason, source);
        self
    }

    /// Sets streamed tool-argument handling.
    #[must_use]
    pub fn with_tool_arguments(mut self, value: ToolArgumentsCompat, source: PolicySource) -> Self {
        self.response.tool_arguments = value;
        self.record_source(CompatField::ResponseToolArguments, source);
        self
    }

    /// Sets streamed usage handling.
    #[must_use]
    pub fn with_usage(mut self, value: UsageCompat, source: PolicySource) -> Self {
        self.response.usage = value;
        self.record_source(CompatField::ResponseUsage, source);
        self
    }

    /// Rejects a contract that contradicts the capabilities it will run under.
    ///
    /// Moved here from the retired `compat/validate.rs` and moved *earlier*: it
    /// now runs when a provider definition is built, not when the first request
    /// is planned, so an inconsistent declaration cannot reach a runtime.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when a strategy requires a capability that
    /// is not explicitly supported.
    pub fn validate_against(
        &self,
        capabilities: &crate::provider::ProviderCapabilities,
    ) -> Result<(), crate::error::LlmError> {
        use crate::domain::{CapabilityStatus, StreamUsagePolicy};

        if matches!(self.request.stream_usage, StreamUsagePolicy::IncludeUsage)
            && !matches!(capabilities.streaming_usage, CapabilityStatus::Supported)
        {
            return Err(crate::error::LlmError::Configuration(
                "stream usage compatibility requires explicit streaming usage support".to_owned(),
            ));
        }
        if matches!(
            self.response.tool_arguments,
            ToolArgumentsCompat::StringOrObject
        ) && !matches!(capabilities.function_tools, CapabilityStatus::Supported)
        {
            return Err(crate::error::LlmError::Configuration(
                "tool argument compatibility requires explicit function tool support".to_owned(),
            ));
        }
        Ok(())
    }
}

impl CompatField {
    const ALL: [Self; 16] = [
        Self::RequestModelBody,
        Self::RequestMaxOutputTokens,
        Self::RequestToolChoice,
        Self::RequestThinking,
        Self::RequestImage,
        Self::RequestStreamUsage,
        Self::RequestStructuredOutput,
        Self::ResponseFinishReason,
        Self::ResponseToolArguments,
        Self::ResponseUsage,
        Self::ResponseInlineError,
        Self::HistoryMissingToolResult,
        Self::HistoryUnsupportedContent,
        Self::HistoryThinkingReplay,
        Self::HistoryToolResultName,
        Self::HistoryToolCallId,
    ];

    /// Returns every stable compatibility leaf in deterministic order.
    #[must_use]
    pub const fn all() -> [Self; 16] {
        Self::ALL
    }
}
