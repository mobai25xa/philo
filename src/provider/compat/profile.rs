//! Resolved typed compatibility profile and field provenance.

use std::collections::BTreeMap;

use crate::domain::{DialectPolicy, PolicySource};

use super::{HistoryCompat, RequestCompat, ResponseCompat};

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

    pub(super) fn parts_mut(
        &mut self,
    ) -> (
        &mut RequestCompat,
        &mut ResponseCompat,
        &mut HistoryCompat,
        &mut BTreeMap<CompatField, PolicySource>,
    ) {
        (
            &mut self.request,
            &mut self.response,
            &mut self.history,
            &mut self.provenance,
        )
    }
}

impl CompatField {
    pub(super) const ALL: [Self; 16] = [
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
