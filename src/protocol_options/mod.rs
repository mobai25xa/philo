//! Protocol-scoped typed options and the bounded dangerous body extension.
//!
//! These options intentionally remain outside the provider-independent domain: a
//! field that only one protocol understands does not belong in a shared request.
//!
//! [`ProtocolOptions`] is a closed protocol-keyed enum. It never degrades into a
//! map or a trait object, and the runtime rejects options whose protocol does not
//! match the selected provider runtime.
//!
//! Each protocol offers one explicitly dangerous raw body extension. Both share a
//! single bounded implementation and a single protection table owner
//! ([`crate::protected`]): only unknown top-level body fields are admitted, and
//! core request fields, headers, credentials, and protocol versions stay protected.

mod anthropic;
mod openai;
mod raw;

pub use anthropic::{
    AnthropicEffort, AnthropicMessagesOptions, AnthropicRawExtension, AnthropicThinkingDisplay,
};
pub use openai::{OpenAiChatOptions, OpenAiChatRawExtension};

use std::fmt;

/// Stable protocol identifier for Anthropic Messages options.
pub const ANTHROPIC_MESSAGES_PROTOCOL_ID: &str = "anthropic-messages";

/// Stable protocol identifier for `OpenAI` Chat Completions options.
pub const OPENAI_CHAT_PROTOCOL_ID: &str = "openai-chat-completions";

/// Value-free diagnostic emitted by protocol-scoped options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtocolOptionDiagnostic {
    /// A dangerous, non-portable raw body extension is active.
    NonPortableExtensionUsed,
}

/// Closed protocol-keyed option container.
#[derive(Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtocolOptions {
    /// Anthropic Messages-only options.
    AnthropicMessages(AnthropicMessagesOptions),
    /// `OpenAI` Chat Completions-only options.
    OpenAiChat(OpenAiChatOptions),
}

impl ProtocolOptions {
    /// Returns the protocol identifier required by these options.
    #[must_use]
    pub const fn protocol_id(&self) -> &'static str {
        match self {
            Self::AnthropicMessages(_) => ANTHROPIC_MESSAGES_PROTOCOL_ID,
            Self::OpenAiChat(_) => OPENAI_CHAT_PROTOCOL_ID,
        }
    }

    /// Returns the value-free label of the active protocol option set.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::AnthropicMessages(_) => "anthropic-messages-options",
            Self::OpenAiChat(_) => "openai-chat-options",
        }
    }

    /// Returns Anthropic options when this is the Anthropic variant.
    #[must_use]
    pub const fn anthropic_messages(&self) -> Option<&AnthropicMessagesOptions> {
        match self {
            Self::AnthropicMessages(options) => Some(options),
            Self::OpenAiChat(_) => None,
        }
    }

    /// Returns `OpenAI` Chat options when this is the `OpenAI` Chat variant.
    #[must_use]
    pub const fn openai_chat(&self) -> Option<&OpenAiChatOptions> {
        match self {
            Self::OpenAiChat(options) => Some(options),
            Self::AnthropicMessages(_) => None,
        }
    }

    /// Returns value-free option diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<ProtocolOptionDiagnostic> {
        match self {
            Self::AnthropicMessages(options) => options.diagnostics(),
            Self::OpenAiChat(options) => options.diagnostics(),
        }
    }
}

impl From<AnthropicMessagesOptions> for ProtocolOptions {
    fn from(value: AnthropicMessagesOptions) -> Self {
        Self::AnthropicMessages(value)
    }
}

impl From<OpenAiChatOptions> for ProtocolOptions {
    fn from(value: OpenAiChatOptions) -> Self {
        Self::OpenAiChat(value)
    }
}

impl fmt::Debug for ProtocolOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnthropicMessages(options) => formatter
                .debug_tuple("AnthropicMessages")
                .field(options)
                .finish(),
            Self::OpenAiChat(options) => {
                formatter.debug_tuple("OpenAiChat").field(options).finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_ids_match_the_identifiers_provider_definitions_register() {
        assert_eq!(
            ProtocolOptions::from(AnthropicMessagesOptions::new()).protocol_id(),
            "anthropic-messages"
        );
        assert_eq!(
            ProtocolOptions::from(OpenAiChatOptions::new()).protocol_id(),
            "openai-chat-completions"
        );
    }

    #[test]
    fn variant_accessors_are_mutually_exclusive() {
        let anthropic = ProtocolOptions::from(AnthropicMessagesOptions::new());
        assert!(anthropic.anthropic_messages().is_some());
        assert!(anthropic.openai_chat().is_none());

        let openai = ProtocolOptions::from(OpenAiChatOptions::new());
        assert!(openai.openai_chat().is_some());
        assert!(openai.anthropic_messages().is_none());
    }

    #[test]
    fn debug_reports_the_variant_without_raw_values() {
        let raw = OpenAiChatRawExtension::dangerous_from_object(
            serde_json::json!({"x": "canary-secret"}),
        )
        .unwrap();
        let options = ProtocolOptions::from(OpenAiChatOptions::new().with_raw_extension(raw));
        let rendered = format!("{options:?}");
        assert!(rendered.contains("OpenAiChat"));
        assert!(!rendered.contains("canary-secret"));
        assert_eq!(
            options.diagnostics(),
            vec![ProtocolOptionDiagnostic::NonPortableExtensionUsed]
        );
    }
}
