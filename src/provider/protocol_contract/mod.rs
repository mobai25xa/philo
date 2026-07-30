//! Fully resolved, protocol-specific behavior fixed when a definition is built.
//!
//! FR-004 retired `provider/compat`. The parts of it that decide *correctness*
//! — which finish reasons are legal, where usage counters live, whether an
//! inline error is an error, what shape tool arguments arrive in — were never
//! optional policy: getting them wrong means a wrong result or inaccurate
//! billing. They belong to the protocol contract, and they are frozen here at
//! definition build time rather than merged per request.
//!
//! Configuration layers, when used, must be resolved before they reach this
//! module.

mod binding;
mod history;
mod profile;
mod request;
mod response;

pub use history::HistoryCompat;
pub use profile::{CompatField, CompatProfile};
pub use request::{MaxOutputTokensWireFormat, ModelBodyWireFormat, RequestCompat};
pub use response::{
    AnthropicUsageCompat, FinishReasonCompat, InlineErrorCompat, ResponseCompat,
    ToolArgumentsCompat, UsageCompat,
};

pub(crate) use binding::ValidatedProtocolBinding;

use std::fmt;

use crate::domain::{
    CapabilityStatus, DialectPolicy, HistoryPolicy, MissingToolResultPolicy, PolicySource,
    ThinkingReplayPolicy, UnsupportedContentPolicy,
};
use crate::error::{LlmError, ValidationError, ValidationReason};
use crate::protocol_options::ProtocolOptions;

use super::capability::ProtocolDialect;

/// Closed, strongly typed protocol contract selected by a provider definition.
#[derive(Clone, Eq, PartialEq)]
pub(crate) enum ResolvedProtocolContract {
    /// `OpenAI` Chat Completions behavior, including reviewed compatibility deviations.
    OpenAiChat(OpenAiChatContract),
    /// Anthropic Messages behavior.
    AnthropicMessages(AnthropicMessagesContract),
}

impl ResolvedProtocolContract {
    pub(crate) fn strict_openai_chat() -> Self {
        Self::OpenAiChat(OpenAiChatContract::strict())
    }

    pub(crate) const fn strict_anthropic_messages() -> Self {
        Self::AnthropicMessages(AnthropicMessagesContract::strict_official())
    }

    pub(crate) const fn matches_dialect(&self, dialect: ProtocolDialect) -> bool {
        matches!(
            (self, dialect),
            (Self::OpenAiChat(_), ProtocolDialect::OpenAiChatCompletions)
                | (
                    Self::AnthropicMessages(_),
                    ProtocolDialect::AnthropicMessages
                )
        )
    }

    pub(crate) const fn label(&self) -> &'static str {
        match self {
            Self::OpenAiChat(_) => "openai-chat",
            Self::AnthropicMessages(_) => "anthropic-messages",
        }
    }

    pub(crate) fn openai_chat(&self) -> Option<&OpenAiChatContract> {
        match self {
            Self::OpenAiChat(contract) => Some(contract),
            Self::AnthropicMessages(_) => None,
        }
    }

    pub(crate) const fn anthropic_messages(&self) -> Option<&AnthropicMessagesContract> {
        match self {
            Self::AnthropicMessages(contract) => Some(contract),
            Self::OpenAiChat(_) => None,
        }
    }

    pub(crate) fn dialect_policy(&self) -> DialectPolicy {
        match self {
            Self::OpenAiChat(contract) => contract.compat().dialect_policy(),
            Self::AnthropicMessages(_) => AnthropicMessagesContract::dialect_policy(),
        }
    }

    pub(crate) fn history_policy(&self) -> HistoryPolicy {
        match self {
            Self::OpenAiChat(contract) => {
                let history = contract.compat().history();
                HistoryPolicy {
                    missing_tool_result: history.missing_tool_result,
                    unsupported_content: history.unsupported_content,
                    thinking_replay: history.thinking_replay,
                }
            }
            Self::AnthropicMessages(contract) => HistoryPolicy {
                missing_tool_result: contract.missing_tool_result,
                unsupported_content: contract.unsupported_content,
                thinking_replay: contract.thinking_replay,
            },
        }
    }

    /// Validates protocol-scoped request options against the resolved contract
    /// and exact model capabilities selected for this call.
    pub(crate) fn validate_options(
        &self,
        options: Option<&ProtocolOptions>,
        capabilities: &super::ProviderCapabilities,
    ) -> Result<(), LlmError> {
        match (self, options) {
            (_, None) | (Self::OpenAiChat(_), Some(ProtocolOptions::OpenAiChat(_))) => Ok(()),
            (Self::AnthropicMessages(_), Some(ProtocolOptions::AnthropicMessages(options))) => {
                AnthropicMessagesContract::validate_options(options, capabilities)
            }
            (Self::OpenAiChat(_), Some(ProtocolOptions::AnthropicMessages(_)))
            | (Self::AnthropicMessages(_), Some(ProtocolOptions::OpenAiChat(_))) => {
                Err(ValidationError::new(
                    "protocol_options",
                    ValidationReason::Conflict,
                    "protocol-scoped options do not match the selected runtime protocol",
                )
                .into())
            }
        }
    }

    /// Rejects a resolved contract that contradicts the capabilities it will
    /// run under, without exposing concrete contract variants to the runtime.
    pub(crate) fn validate_capabilities(
        &self,
        capabilities: &super::ProviderCapabilities,
    ) -> Result<(), LlmError> {
        match self {
            Self::OpenAiChat(contract) => contract.compat().validate_against(capabilities),
            Self::AnthropicMessages(_) => Ok(()),
        }
    }

    /// Returns the stable source used by the plan's protocol provenance summary.
    pub(crate) fn provenance_source(&self) -> PolicySource {
        match self {
            Self::OpenAiChat(contract) => contract
                .compat()
                .source(CompatField::RequestMaxOutputTokens),
            Self::AnthropicMessages(_) => PolicySource::ProtocolDefault,
        }
    }
}

impl fmt::Debug for ResolvedProtocolContract {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ResolvedProtocolContract")
            .field(&self.label())
            .finish()
    }
}

/// Complete `OpenAI` Chat Completions contract for one call.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct OpenAiChatContract {
    compat: CompatProfile,
}

impl OpenAiChatContract {
    pub(crate) fn strict() -> Self {
        Self {
            compat: CompatProfile::openai_chat_default(),
        }
    }

    pub(crate) const fn from_compat(compat: CompatProfile) -> Self {
        Self { compat }
    }

    pub(crate) const fn compat(&self) -> &CompatProfile {
        &self.compat
    }
}

/// Ownership rule for the required `anthropic-version` header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicVersionHeaderPolicy {
    /// The provider definition must install and protect the header.
    ProviderRequired,
}

/// Ownership rule for the optional `anthropic-beta` header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicBetaHeaderPolicy {
    /// The provider definition owns the header and may explicitly remove it.
    ProviderControlled,
}

/// Required successful terminal event behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicTerminalPolicy {
    /// A successful stream must end with `message_stop`.
    RequireMessageStop,
}

/// Unknown-event and event/type mismatch behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicEventPolicy {
    /// Ignore unknown event types but reject a known event whose JSON type differs.
    IgnoreUnknownRejectMismatch,
}

/// Tool input block completion behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicToolBlockPolicy {
    /// A tool block must close with complete, valid JSON.
    RequireCompleteJson,
}

/// Thinking/signature/redacted replay behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicThinkingPolicy {
    /// Preserve opaque state only for same-source replay and never expose it as answer text.
    SameSourceOpaque,
}

/// HTTP and stream error-envelope expectation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicErrorEnvelopePolicy {
    /// Decode the official typed envelope and retain only safe code/request-id metadata.
    TypedOfficial,
}

/// Versioned system/history mapping semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicHistorySemantics {
    /// Top-level system blocks plus alternating user/assistant Messages history.
    Messages2023,
}

/// Complete Anthropic Messages contract for one call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AnthropicMessagesContract {
    pub(crate) version_header: AnthropicVersionHeaderPolicy,
    pub(crate) beta_header: AnthropicBetaHeaderPolicy,
    pub(crate) terminal: AnthropicTerminalPolicy,
    pub(crate) events: AnthropicEventPolicy,
    pub(crate) usage: AnthropicUsageCompat,
    pub(crate) tool_blocks: AnthropicToolBlockPolicy,
    pub(crate) thinking: AnthropicThinkingPolicy,
    pub(crate) error_envelope: AnthropicErrorEnvelopePolicy,
    pub(crate) history_semantics: AnthropicHistorySemantics,
    pub(crate) missing_tool_result: MissingToolResultPolicy,
    pub(crate) unsupported_content: UnsupportedContentPolicy,
    pub(crate) thinking_replay: ThinkingReplayPolicy,
}

impl AnthropicMessagesContract {
    pub(crate) const fn strict_official() -> Self {
        Self {
            version_header: AnthropicVersionHeaderPolicy::ProviderRequired,
            beta_header: AnthropicBetaHeaderPolicy::ProviderControlled,
            terminal: AnthropicTerminalPolicy::RequireMessageStop,
            events: AnthropicEventPolicy::IgnoreUnknownRejectMismatch,
            usage: AnthropicUsageCompat::StrictStableFields,
            tool_blocks: AnthropicToolBlockPolicy::RequireCompleteJson,
            thinking: AnthropicThinkingPolicy::SameSourceOpaque,
            error_envelope: AnthropicErrorEnvelopePolicy::TypedOfficial,
            history_semantics: AnthropicHistorySemantics::Messages2023,
            missing_tool_result: MissingToolResultPolicy::Reject,
            unsupported_content: UnsupportedContentPolicy::Reject,
            thinking_replay: ThinkingReplayPolicy::SameSourceOnly,
        }
    }

    pub(crate) const fn dialect_policy() -> DialectPolicy {
        DialectPolicy::official_anthropic()
    }

    pub(crate) const fn with_usage_compat(mut self, usage: AnthropicUsageCompat) -> Self {
        self.usage = usage;
        self
    }

    fn validate_options(
        options: &crate::protocol_options::AnthropicMessagesOptions,
        capabilities: &super::ProviderCapabilities,
    ) -> Result<(), LlmError> {
        if options.adaptive_thinking().is_some() {
            validate_protocol_capability(
                "protocol_options.anthropic.adaptive_thinking",
                capabilities.adaptive_thinking,
            )?;
        }
        if options.effort().is_some() {
            validate_protocol_capability(
                "protocol_options.anthropic.effort",
                capabilities.adaptive_thinking_effort,
            )?;
        }
        Ok(())
    }
}

fn validate_protocol_capability(
    field: &'static str,
    status: CapabilityStatus,
) -> Result<(), LlmError> {
    match status {
        CapabilityStatus::Supported => Ok(()),
        CapabilityStatus::Unsupported => Err(ValidationError::new(
            field,
            ValidationReason::CapabilityUnsupported,
            "selected model does not support this protocol-scoped option",
        )
        .into()),
        CapabilityStatus::Unknown => Err(ValidationError::new(
            field,
            ValidationReason::CapabilityUnknown,
            "selected model support for this protocol-scoped option is unknown",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{CapabilityStatus, PolicySource};
    use crate::error::{LlmError, ValidationReason};
    use crate::protocol_options::{
        AnthropicEffort, AnthropicMessagesOptions, AnthropicThinkingDisplay, OpenAiChatOptions,
    };

    use super::{CompatField, ResolvedProtocolContract};

    #[test]
    fn anthropic_contract_owns_option_capability_errors_and_priority() {
        let contract = ResolvedProtocolContract::strict_anthropic_messages();
        let options = AnthropicMessagesOptions::new()
            .with_adaptive_thinking(AnthropicThinkingDisplay::Omitted)
            .with_effort(AnthropicEffort::High)
            .into();
        let mut capabilities = crate::provider::ProviderCapabilities::conservative_messages();
        capabilities.adaptive_thinking = CapabilityStatus::Unsupported;
        capabilities.adaptive_thinking_effort = CapabilityStatus::Unknown;

        let error = contract
            .validate_options(Some(&options), &capabilities)
            .unwrap_err();
        assert!(matches!(
            error,
            LlmError::Validation(ref error)
                if error.field() == "protocol_options.anthropic.adaptive_thinking"
                    && error.reason() == ValidationReason::CapabilityUnsupported
        ));
    }

    #[test]
    fn contract_rejects_wrong_options_before_variant_specific_policy() {
        let contract = ResolvedProtocolContract::strict_anthropic_messages();
        let options = OpenAiChatOptions::new().into();
        let capabilities = crate::provider::ProviderCapabilities::conservative_messages();

        let error = contract
            .validate_options(Some(&options), &capabilities)
            .unwrap_err();
        assert!(matches!(
            error,
            LlmError::Validation(ref error)
                if error.field() == "protocol_options"
                    && error.reason() == ValidationReason::Conflict
        ));
    }

    #[test]
    fn protocol_provenance_summary_is_variant_neutral_to_callers() {
        let compat = super::CompatProfile::openai_chat_default().with_max_output_tokens(
            super::MaxOutputTokensWireFormat::MaxTokens,
            PolicySource::ModelProfile,
        );
        let openai =
            ResolvedProtocolContract::OpenAiChat(super::OpenAiChatContract::from_compat(compat));
        assert_eq!(openai.provenance_source(), PolicySource::ModelProfile);
        assert_eq!(
            openai
                .openai_chat()
                .unwrap()
                .compat()
                .source(CompatField::RequestMaxOutputTokens),
            PolicySource::ModelProfile
        );
        assert_eq!(
            ResolvedProtocolContract::strict_anthropic_messages().provenance_source(),
            PolicySource::ProtocolDefault
        );
    }

    #[test]
    fn openai_contract_owns_resolved_compat_capability_validation() {
        let compat = super::CompatProfile::openai_chat_default().with_tool_arguments(
            super::ToolArgumentsCompat::StringOrObject,
            PolicySource::ProviderProfile,
        );
        let contract =
            ResolvedProtocolContract::OpenAiChat(super::OpenAiChatContract::from_compat(compat));
        let capabilities = crate::provider::ProviderCapabilities::conservative_chat_completions();

        assert!(matches!(
            contract.validate_capabilities(&capabilities),
            Err(LlmError::Configuration(message))
                if message == "tool argument compatibility requires explicit function tool support"
        ));
    }
}
