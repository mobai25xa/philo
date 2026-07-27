//! Deterministic fieldwise compatibility merge.

use std::collections::BTreeMap;

use philo::domain::{
    ImageWireFormat, MissingToolResultPolicy, PolicySource, StreamUsagePolicy,
    StructuredOutputWireFormat, ThinkingReplayPolicy, ThinkingWireFormat, ToolCallIdPolicy,
    ToolChoiceWireFormat, ToolResultNamePolicy, UnsupportedContentPolicy,
};
use philo::provider::{
    CompatField, CompatProfile, FinishReasonCompat, InlineErrorCompat, MaxOutputTokensWireFormat,
    ModelBodyWireFormat, ToolArgumentsCompat, UsageCompat,
};

/// Sparse typed overrides applied at one precedence layer.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompatPatch {
    /// Layer source assigned to every present leaf.
    pub source: Option<PolicySource>,
    /// Per-leaf sources retained after patches are combined.
    pub provenance: BTreeMap<CompatField, PolicySource>,
    /// Request model field presence.
    pub request_model_body: Option<ModelBodyWireFormat>,
    /// Maximum output token request field.
    pub request_max_output_tokens: Option<MaxOutputTokensWireFormat>,
    /// Request tool-choice format.
    pub request_tool_choice: Option<ToolChoiceWireFormat>,
    /// Request thinking format.
    pub request_thinking: Option<ThinkingWireFormat>,
    /// Request image format.
    pub request_image: Option<ImageWireFormat>,
    /// Stream usage request policy.
    pub request_stream_usage: Option<StreamUsagePolicy>,
    /// Structured-output request format.
    pub request_structured_output: Option<StructuredOutputWireFormat>,
    /// Finish-reason handling.
    pub response_finish_reason: Option<FinishReasonCompat>,
    /// Tool-argument handling.
    pub response_tool_arguments: Option<ToolArgumentsCompat>,
    /// Usage handling.
    pub response_usage: Option<UsageCompat>,
    /// Inline error handling.
    pub response_inline_error: Option<InlineErrorCompat>,
    /// Missing tool-result behavior.
    pub history_missing_tool_result: Option<MissingToolResultPolicy>,
    /// Unsupported content behavior.
    pub history_unsupported_content: Option<UnsupportedContentPolicy>,
    /// Thinking replay behavior.
    pub history_thinking_replay: Option<ThinkingReplayPolicy>,
    /// Tool-result name behavior.
    pub history_tool_result_name: Option<ToolResultNamePolicy>,
    /// Tool-call id behavior.
    pub history_tool_call_id: Option<ToolCallIdPolicy>,
}

impl CompatPatch {
    /// Creates an empty patch for one explicit precedence source.
    #[must_use]
    pub const fn from_source(source: PolicySource) -> Self {
        Self {
            source: Some(source),
            provenance: BTreeMap::new(),
            request_model_body: None,
            request_max_output_tokens: None,
            request_tool_choice: None,
            request_thinking: None,
            request_image: None,
            request_stream_usage: None,
            request_structured_output: None,
            response_finish_reason: None,
            response_tool_arguments: None,
            response_usage: None,
            response_inline_error: None,
            history_missing_tool_result: None,
            history_unsupported_content: None,
            history_thinking_replay: None,
            history_tool_result_name: None,
            history_tool_call_id: None,
        }
    }

    /// Sets whether the request body carries the resolved wire model.
    #[must_use]
    pub const fn with_model_body(mut self, value: ModelBodyWireFormat) -> Self {
        self.request_model_body = Some(value);
        self
    }

    /// Sets maximum output token wire format.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, value: MaxOutputTokensWireFormat) -> Self {
        self.request_max_output_tokens = Some(value);
        self
    }

    /// Sets streamed finish-reason handling.
    #[must_use]
    pub const fn with_finish_reason(mut self, value: FinishReasonCompat) -> Self {
        self.response_finish_reason = Some(value);
        self
    }

    /// Sets streamed tool-argument handling.
    #[must_use]
    pub const fn with_tool_arguments(mut self, value: ToolArgumentsCompat) -> Self {
        self.response_tool_arguments = Some(value);
        self
    }

    /// Sets streamed usage handling.
    #[must_use]
    pub const fn with_usage(mut self, value: UsageCompat) -> Self {
        self.response_usage = Some(value);
        self
    }

    /// Reports whether the patch contains no policy leaf.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.request_model_body.is_none()
            && self.request_max_output_tokens.is_none()
            && self.request_tool_choice.is_none()
            && self.request_thinking.is_none()
            && self.request_image.is_none()
            && self.request_stream_usage.is_none()
            && self.request_structured_output.is_none()
            && self.response_finish_reason.is_none()
            && self.response_tool_arguments.is_none()
            && self.response_usage.is_none()
            && self.response_inline_error.is_none()
            && self.history_missing_tool_result.is_none()
            && self.history_unsupported_content.is_none()
            && self.history_thinking_replay.is_none()
            && self.history_tool_result_name.is_none()
            && self.history_tool_call_id.is_none()
    }

    /// Overlays a higher-precedence patch onto this one.
    pub fn overlay(&mut self, later: &Self) {
        macro_rules! overlay {
            ($field:ident, $compat_field:expr) => {
                if later.$field.is_some() {
                    self.$field = later.$field;
                    self.provenance
                        .insert($compat_field, later.source_for($compat_field));
                }
            };
        }
        overlay!(request_model_body, CompatField::RequestModelBody);
        overlay!(
            request_max_output_tokens,
            CompatField::RequestMaxOutputTokens
        );
        overlay!(request_tool_choice, CompatField::RequestToolChoice);
        overlay!(request_thinking, CompatField::RequestThinking);
        overlay!(request_image, CompatField::RequestImage);
        overlay!(request_stream_usage, CompatField::RequestStreamUsage);
        overlay!(
            request_structured_output,
            CompatField::RequestStructuredOutput
        );
        overlay!(response_finish_reason, CompatField::ResponseFinishReason);
        overlay!(response_tool_arguments, CompatField::ResponseToolArguments);
        overlay!(response_usage, CompatField::ResponseUsage);
        overlay!(response_inline_error, CompatField::ResponseInlineError);
        overlay!(
            history_missing_tool_result,
            CompatField::HistoryMissingToolResult
        );
        overlay!(
            history_unsupported_content,
            CompatField::HistoryUnsupportedContent
        );
        overlay!(history_thinking_replay, CompatField::HistoryThinkingReplay);
        overlay!(history_tool_result_name, CompatField::HistoryToolResultName);
        overlay!(history_tool_call_id, CompatField::HistoryToolCallId);
    }

    /// Applies this patch to a resolved contract.
    pub fn apply_to(&self, profile: &mut CompatProfile) {
        let mut sources = Vec::new();
        let (request, response, history) = profile.parts_mut();
        macro_rules! apply {
            ($slot:expr, $value:expr, $field:expr) => {
                if let Some(value) = $value {
                    $slot = value;
                    sources.push($field);
                }
            };
        }
        apply!(
            request.model_body,
            self.request_model_body,
            CompatField::RequestModelBody
        );
        apply!(
            request.max_output_tokens,
            self.request_max_output_tokens,
            CompatField::RequestMaxOutputTokens
        );
        apply!(
            request.tool_choice,
            self.request_tool_choice,
            CompatField::RequestToolChoice
        );
        apply!(
            request.thinking,
            self.request_thinking,
            CompatField::RequestThinking
        );
        apply!(request.image, self.request_image, CompatField::RequestImage);
        apply!(
            request.stream_usage,
            self.request_stream_usage,
            CompatField::RequestStreamUsage
        );
        apply!(
            request.structured_output,
            self.request_structured_output,
            CompatField::RequestStructuredOutput
        );
        apply!(
            response.finish_reason,
            self.response_finish_reason,
            CompatField::ResponseFinishReason
        );
        apply!(
            response.tool_arguments,
            self.response_tool_arguments,
            CompatField::ResponseToolArguments
        );
        apply!(
            response.usage,
            self.response_usage,
            CompatField::ResponseUsage
        );
        apply!(
            response.inline_error,
            self.response_inline_error,
            CompatField::ResponseInlineError
        );
        apply!(
            history.missing_tool_result,
            self.history_missing_tool_result,
            CompatField::HistoryMissingToolResult
        );
        apply!(
            history.unsupported_content,
            self.history_unsupported_content,
            CompatField::HistoryUnsupportedContent
        );
        apply!(
            history.thinking_replay,
            self.history_thinking_replay,
            CompatField::HistoryThinkingReplay
        );
        apply!(
            history.tool_result_name,
            self.history_tool_result_name,
            CompatField::HistoryToolResultName
        );
        apply!(
            history.tool_call_id,
            self.history_tool_call_id,
            CompatField::HistoryToolCallId
        );
        for field in sources {
            profile.record_source(field, self.source_for(field));
        }
    }

    fn source_for(&self, field: CompatField) -> PolicySource {
        self.provenance
            .get(&field)
            .copied()
            .or(self.source)
            .unwrap_or(PolicySource::ProviderProfile)
    }
}

/// Resolves protocol defaults followed by ordered sparse layers.
///
/// The result is what the core accepts: one immutable contract, with every
/// leaf attributed to the layer that set it.
#[must_use]
pub fn resolve_compat(layers: &[CompatPatch]) -> CompatProfile {
    let mut profile = CompatProfile::openai_chat_default();
    for layer in layers {
        layer.apply_to(&mut profile);
    }
    profile
}
