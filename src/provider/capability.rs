//! Provider capability, dialect, and transport safety declarations.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use crate::domain::{CapabilitySet, CapabilityStatus, ModelId, ReasoningEffortSupport};
use crate::error::LlmError;
use crate::provider::catalog::CatalogCapabilities;
use crate::provider::endpoint::RedirectPolicy;

/// Date on which official phase-two capability declarations were last reviewed.
pub const OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE: &str = "2026-07-19";

/// Date on which the official Anthropic Messages capability declarations were reviewed.
pub const OFFICIAL_ANTHROPIC_CAPABILITY_REVIEW_DATE: &str = "2026-07-25";

/// Provider defaults before an exact model capability profile is applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCapabilities {
    /// Developer-role message support.
    pub developer_role: CapabilityStatus,
    /// Temperature option support.
    pub temperature: CapabilityStatus,
    /// Maximum completion token option support.
    pub max_completion_tokens: CapabilityStatus,
    /// SSE streaming support.
    pub streaming: CapabilityStatus,
    /// Streaming usage support.
    pub streaming_usage: CapabilityStatus,
    /// Function tool support.
    pub function_tools: CapabilityStatus,
    /// Required tool-choice support.
    pub tool_choice_required: CapabilityStatus,
    /// Specific function tool-choice support.
    pub tool_choice_specific: CapabilityStatus,
    /// Parallel tool-call support.
    pub parallel_tool_calls: CapabilityStatus,
    /// Strict function-schema support.
    pub strict_tools: CapabilityStatus,
    /// Image input support.
    pub vision_input: CapabilityStatus,
    /// Original image-detail support.
    pub image_detail_original: CapabilityStatus,
    /// JSON object response-format support.
    pub response_format_json_object: CapabilityStatus,
    /// JSON schema response-format support.
    pub response_format_json_schema: CapabilityStatus,
    /// Exact reasoning efforts supported by an exact model profile.
    pub reasoning_efforts: ReasoningEffortSupport,
    /// Protocol-scoped adaptive-thinking request support.
    pub adaptive_thinking: CapabilityStatus,
    /// Protocol-scoped adaptive-thinking effort support.
    pub adaptive_thinking_effort: CapabilityStatus,
}

impl ProviderCapabilities {
    /// Returns a conservative custom `OpenAI` Chat declaration.
    ///
    /// Only core text generation, token limits, and streaming are declared;
    /// tools, images, structured output, and reasoning remain unknown.
    pub fn conservative_chat_completions() -> Self {
        Self::openai_compatible()
    }

    /// Returns a conservative custom Anthropic Messages declaration.
    ///
    /// Only core text generation, token limits, and streaming are declared;
    /// tools, images, structured output, and thinking remain unknown.
    pub fn conservative_messages() -> Self {
        Self {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unsupported,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unsupported,
            adaptive_thinking: CapabilityStatus::Unknown,
            adaptive_thinking_effort: CapabilityStatus::Unknown,
        }
    }

    /// Returns the subset used by domain request validation.
    pub fn generation_options(&self) -> CapabilitySet {
        CapabilitySet {
            temperature: self.temperature,
            max_output_tokens: self.max_completion_tokens,
            function_tools: self.function_tools,
            tool_choice_required: self.tool_choice_required,
            tool_choice_specific: self.tool_choice_specific,
            parallel_tool_calls: self.parallel_tool_calls,
            strict_tools: self.strict_tools,
            vision_input: self.vision_input,
            image_detail_original: self.image_detail_original,
            response_format_json_object: self.response_format_json_object,
            response_format_json_schema: self.response_format_json_schema,
            reasoning_efforts: self.reasoning_efforts.clone(),
        }
    }

    pub(crate) fn official_openai() -> Self {
        Self {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
            adaptive_thinking: CapabilityStatus::Unsupported,
            adaptive_thinking_effort: CapabilityStatus::Unsupported,
        }
    }

    pub(super) fn official_anthropic() -> Self {
        Self {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Supported,
            tool_choice_required: CapabilityStatus::Supported,
            tool_choice_specific: CapabilityStatus::Supported,
            parallel_tool_calls: CapabilityStatus::Supported,
            strict_tools: CapabilityStatus::Supported,
            vision_input: CapabilityStatus::Supported,
            image_detail_original: CapabilityStatus::Unsupported,
            response_format_json_object: CapabilityStatus::Unsupported,
            response_format_json_schema: CapabilityStatus::Supported,
            reasoning_efforts: ReasoningEffortSupport::Unsupported,
            adaptive_thinking: CapabilityStatus::Unknown,
            adaptive_thinking_effort: CapabilityStatus::Unknown,
        }
    }

    /// Conservative OpenAI-compatible base used by reviewed third-party presets.
    pub(super) fn openai_compatible() -> Self {
        Self {
            developer_role: CapabilityStatus::Unknown,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
            adaptive_thinking: CapabilityStatus::Unsupported,
            adaptive_thinking_effort: CapabilityStatus::Unsupported,
        }
    }

    pub(super) fn validate(&self) -> Result<(), LlmError> {
        if matches!(
            self.streaming,
            CapabilityStatus::Unsupported | CapabilityStatus::Unknown
        ) {
            return Err(configuration("profile must declare streaming support"));
        }
        if matches!(
            self.streaming_usage,
            CapabilityStatus::Unsupported | CapabilityStatus::Unknown
        ) {
            return Err(configuration(
                "profile must declare streaming usage support",
            ));
        }
        Ok(())
    }

    pub(super) fn apply_model(&mut self, profile: &ModelCapabilityProfile) {
        self.function_tools = profile.function_tools;
        self.tool_choice_required = profile.tool_choice_required;
        self.tool_choice_specific = profile.tool_choice_specific;
        self.parallel_tool_calls = profile.parallel_tool_calls;
        self.strict_tools = profile.strict_tools;
        self.vision_input = profile.vision_input;
        self.image_detail_original = profile.image_detail_original;
        self.response_format_json_object = profile.response_format_json_object;
        self.response_format_json_schema = profile.response_format_json_schema;
        self.reasoning_efforts = profile.reasoning_efforts.clone();
        self.adaptive_thinking = profile.adaptive_thinking;
        self.adaptive_thinking_effort = profile.adaptive_thinking_effort;
    }

    pub(super) fn apply_catalog(&mut self, profile: &CatalogCapabilities) {
        self.function_tools = profile.function_tools;
        self.tool_choice_required = profile.tool_choice_required;
        self.tool_choice_specific = profile.tool_choice_specific;
        self.parallel_tool_calls = profile.parallel_tool_calls;
        self.strict_tools = profile.strict_tools;
        self.vision_input = profile.vision_input;
        self.image_detail_original = profile.image_detail_original;
        self.response_format_json_object = profile.response_format_json_object;
        self.response_format_json_schema = profile.response_format_json_schema;
        self.reasoning_efforts = profile.reasoning_efforts.clone();
        self.adaptive_thinking = profile.adaptive_thinking;
        self.adaptive_thinking_effort = profile.adaptive_thinking_effort;
    }
}

/// P2 capabilities declared for one exact [`ModelId`].
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilityProfile {
    model: ModelId,
    function_tools: CapabilityStatus,
    tool_choice_required: CapabilityStatus,
    tool_choice_specific: CapabilityStatus,
    parallel_tool_calls: CapabilityStatus,
    strict_tools: CapabilityStatus,
    vision_input: CapabilityStatus,
    image_detail_original: CapabilityStatus,
    response_format_json_object: CapabilityStatus,
    response_format_json_schema: CapabilityStatus,
    reasoning_efforts: ReasoningEffortSupport,
    adaptive_thinking: CapabilityStatus,
    adaptive_thinking_effort: CapabilityStatus,
}

impl ModelCapabilityProfile {
    /// Creates an exact model profile whose P2 capabilities are all unknown.
    pub fn new(model: ModelId) -> Self {
        Self {
            model,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
            adaptive_thinking: CapabilityStatus::Unknown,
            adaptive_thinking_effort: CapabilityStatus::Unknown,
        }
    }

    /// Returns the exact model identifier.
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// Sets function tool support.
    pub fn with_function_tools(mut self, status: CapabilityStatus) -> Self {
        self.function_tools = status;
        self
    }

    /// Sets required tool-choice support.
    pub fn with_tool_choice_required(mut self, status: CapabilityStatus) -> Self {
        self.tool_choice_required = status;
        self
    }

    /// Sets specific tool-choice support.
    pub fn with_tool_choice_specific(mut self, status: CapabilityStatus) -> Self {
        self.tool_choice_specific = status;
        self
    }

    /// Sets parallel tool-call support.
    pub fn with_parallel_tool_calls(mut self, status: CapabilityStatus) -> Self {
        self.parallel_tool_calls = status;
        self
    }

    /// Sets strict function-schema support.
    pub fn with_strict_tools(mut self, status: CapabilityStatus) -> Self {
        self.strict_tools = status;
        self
    }

    /// Sets image input support.
    pub fn with_vision_input(mut self, status: CapabilityStatus) -> Self {
        self.vision_input = status;
        self
    }

    /// Sets original image-detail support.
    pub fn with_image_detail_original(mut self, status: CapabilityStatus) -> Self {
        self.image_detail_original = status;
        self
    }

    /// Sets JSON object response-format support.
    pub fn with_response_format_json_object(mut self, status: CapabilityStatus) -> Self {
        self.response_format_json_object = status;
        self
    }

    /// Sets JSON schema response-format support.
    pub fn with_response_format_json_schema(mut self, status: CapabilityStatus) -> Self {
        self.response_format_json_schema = status;
        self
    }

    /// Sets the exact reasoning effort declaration.
    pub fn with_reasoning_efforts(mut self, support: ReasoningEffortSupport) -> Self {
        self.reasoning_efforts = support;
        self
    }

    /// Sets adaptive-thinking request support.
    pub fn with_adaptive_thinking(mut self, status: CapabilityStatus) -> Self {
        self.adaptive_thinking = status;
        self
    }

    /// Sets adaptive-thinking effort support.
    pub fn with_adaptive_thinking_effort(mut self, status: CapabilityStatus) -> Self {
        self.adaptive_thinking_effort = status;
        self
    }

    /// Returns function tool support.
    pub fn function_tools(&self) -> CapabilityStatus {
        self.function_tools
    }

    /// Returns required tool-choice support.
    pub fn tool_choice_required(&self) -> CapabilityStatus {
        self.tool_choice_required
    }

    /// Returns specific tool-choice support.
    pub fn tool_choice_specific(&self) -> CapabilityStatus {
        self.tool_choice_specific
    }

    /// Returns parallel tool-call support.
    pub fn parallel_tool_calls(&self) -> CapabilityStatus {
        self.parallel_tool_calls
    }

    /// Returns strict function-schema support.
    pub fn strict_tools(&self) -> CapabilityStatus {
        self.strict_tools
    }

    /// Returns image input support.
    pub fn vision_input(&self) -> CapabilityStatus {
        self.vision_input
    }

    /// Returns original image-detail support.
    pub fn image_detail_original(&self) -> CapabilityStatus {
        self.image_detail_original
    }

    /// Returns JSON object response-format support.
    pub fn response_format_json_object(&self) -> CapabilityStatus {
        self.response_format_json_object
    }

    /// Returns JSON schema response-format support.
    pub fn response_format_json_schema(&self) -> CapabilityStatus {
        self.response_format_json_schema
    }

    /// Returns the reasoning effort declaration.
    pub fn reasoning_efforts(&self) -> &ReasoningEffortSupport {
        &self.reasoning_efforts
    }
}

/// Protocol-specific response/request behavior selected by a profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolDialect {
    /// Official `OpenAI` Chat Completions semantics.
    OpenAiChatCompletions,
    /// Official Anthropic Messages semantics.
    AnthropicMessages,
}

/// Transport safety options owned by a provider profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProviderTransportOptions {
    redirect_policy: RedirectPolicy,
}

impl ProviderTransportOptions {
    /// Creates transport options with redirects disabled.
    pub fn secure_defaults() -> Self {
        Self::default()
    }

    /// Returns redirect policy.
    pub fn redirect_policy(self) -> RedirectPolicy {
        self.redirect_policy
    }
}

fn configuration(message: &'static str) -> LlmError {
    LlmError::Configuration(message.to_owned())
}
