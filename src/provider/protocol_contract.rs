//! Fully resolved, protocol-specific behavior carried by every logical call.

use std::fmt;

use crate::domain::{
    DialectPolicy, HistoryPolicy, MissingToolResultPolicy, ThinkingReplayPolicy,
    UnsupportedContentPolicy,
};

use super::capability::ProtocolDialect;
use super::compat::CompatProfile;

/// Closed, strongly typed protocol contract selected by a provider profile.
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

    pub(crate) fn history_policy(
        &self,
        max_messages: usize,
        max_total_text_bytes: usize,
    ) -> HistoryPolicy {
        match self {
            Self::OpenAiChat(contract) => {
                let history = contract.compat().history();
                HistoryPolicy {
                    missing_tool_result: history.missing_tool_result,
                    unsupported_content: history.unsupported_content,
                    thinking_replay: history.thinking_replay,
                    max_messages,
                    max_total_text_bytes,
                }
            }
            Self::AnthropicMessages(contract) => HistoryPolicy {
                missing_tool_result: contract.missing_tool_result,
                unsupported_content: contract.unsupported_content,
                thinking_replay: contract.thinking_replay,
                max_messages,
                max_total_text_bytes,
            },
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

/// Usage aggregation behavior across Messages events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnthropicUsagePolicy {
    /// Merge cumulative start/delta snapshots and reject regressions.
    MonotonicSnapshots,
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
    pub(crate) usage: AnthropicUsagePolicy,
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
            usage: AnthropicUsagePolicy::MonotonicSnapshots,
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
}
