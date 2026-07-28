//! Atomic, validated protocol identity shared by provider construction stages.

use std::fmt;

use crate::domain::ProtocolId;
use crate::error::LlmError;
use crate::plan::ProtocolKind;

use super::ResolvedProtocolContract;
use crate::provider::capability::ProtocolDialect;

/// A complete protocol identity whose fields cannot drift independently.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ValidatedProtocolBinding {
    id: ProtocolId,
    kind: ProtocolKind,
    dialect: ProtocolDialect,
    contract: ResolvedProtocolContract,
}

impl ValidatedProtocolBinding {
    /// Validates the complete tuple and derives the only admissible dispatch kind.
    pub(crate) fn new(
        id: ProtocolId,
        dialect: ProtocolDialect,
        contract: ResolvedProtocolContract,
    ) -> Result<Self, LlmError> {
        let kind = match (id.as_str(), dialect, &contract) {
            (
                "openai-chat-completions",
                ProtocolDialect::OpenAiChatCompletions,
                ResolvedProtocolContract::OpenAiChat(_),
            ) => ProtocolKind::OpenAiChatCompletions,
            (
                "anthropic-messages",
                ProtocolDialect::AnthropicMessages,
                ResolvedProtocolContract::AnthropicMessages(_),
            ) => ProtocolKind::AnthropicMessages,
            _ => {
                return Err(LlmError::Configuration(
                    "protocol id, dialect, and contract do not form a supported binding".to_owned(),
                ));
            }
        };
        Ok(Self {
            id,
            kind,
            dialect,
            contract,
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn openai_chat() -> Self {
        Self::new(
            ProtocolId::new("openai-chat-completions")
                .expect("static protocol ID must remain valid"),
            ProtocolDialect::OpenAiChatCompletions,
            ResolvedProtocolContract::strict_openai_chat(),
        )
        .expect("static OpenAI protocol binding must remain valid")
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn anthropic_messages() -> Self {
        Self::new(
            ProtocolId::new("anthropic-messages").expect("static protocol ID must remain valid"),
            ProtocolDialect::AnthropicMessages,
            ResolvedProtocolContract::strict_anthropic_messages(),
        )
        .expect("static Anthropic protocol binding must remain valid")
    }

    pub(crate) const fn id(&self) -> &ProtocolId {
        &self.id
    }

    pub(crate) const fn kind(&self) -> ProtocolKind {
        self.kind
    }

    pub(crate) const fn dialect(&self) -> ProtocolDialect {
        self.dialect
    }

    pub(crate) const fn contract(&self) -> &ResolvedProtocolContract {
        &self.contract
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn with_contract(
        self,
        contract: ResolvedProtocolContract,
    ) -> Result<Self, LlmError> {
        Self::new(self.id, self.dialect, contract)
    }

    pub(crate) fn validate_contract(
        &self,
        contract: &ResolvedProtocolContract,
    ) -> Result<(), LlmError> {
        if contract.matches_dialect(self.dialect) {
            Ok(())
        } else {
            Err(LlmError::Configuration(
                "model protocol contract does not match the provider protocol binding".to_owned(),
            ))
        }
    }
}

impl fmt::Debug for ValidatedProtocolBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedProtocolBinding")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("dialect", &self.dialect)
            .field("contract", &self.contract.label())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_openai_binding_exposes_one_consistent_identity() {
        let binding = ValidatedProtocolBinding::openai_chat();
        assert_eq!(binding.id().as_str(), "openai-chat-completions");
        assert_eq!(binding.kind(), ProtocolKind::OpenAiChatCompletions);
        assert_eq!(binding.dialect(), ProtocolDialect::OpenAiChatCompletions);
        assert!(matches!(
            binding.contract(),
            ResolvedProtocolContract::OpenAiChat(_)
        ));
    }

    #[test]
    fn valid_anthropic_binding_exposes_one_consistent_identity() {
        let binding = ValidatedProtocolBinding::anthropic_messages();
        assert_eq!(binding.id().as_str(), "anthropic-messages");
        assert_eq!(binding.kind(), ProtocolKind::AnthropicMessages);
        assert_eq!(binding.dialect(), ProtocolDialect::AnthropicMessages);
        assert!(matches!(
            binding.contract(),
            ResolvedProtocolContract::AnthropicMessages(_)
        ));
    }

    #[test]
    fn every_id_dialect_contract_kind_mismatch_is_rejected() {
        let ids = [
            "openai-chat-completions",
            "anthropic-messages",
            "unsupported-protocol",
        ];
        let dialects = [
            ProtocolDialect::OpenAiChatCompletions,
            ProtocolDialect::AnthropicMessages,
        ];
        for id in ids {
            for dialect in dialects {
                for contract in [
                    ResolvedProtocolContract::strict_openai_chat(),
                    ResolvedProtocolContract::strict_anthropic_messages(),
                ] {
                    let expected_valid = matches!(
                        (id, dialect, &contract),
                        (
                            "openai-chat-completions",
                            ProtocolDialect::OpenAiChatCompletions,
                            ResolvedProtocolContract::OpenAiChat(_)
                        ) | (
                            "anthropic-messages",
                            ProtocolDialect::AnthropicMessages,
                            ResolvedProtocolContract::AnthropicMessages(_)
                        )
                    );
                    let actual = ValidatedProtocolBinding::new(
                        ProtocolId::new(id).unwrap(),
                        dialect,
                        contract,
                    );
                    assert_eq!(actual.is_ok(), expected_valid);
                }
            }
        }
    }

    #[test]
    fn model_contract_override_must_match_binding_dialect() {
        let binding = ValidatedProtocolBinding::openai_chat();
        assert!(
            binding
                .validate_contract(&ResolvedProtocolContract::strict_openai_chat())
                .is_ok()
        );
        assert!(
            binding
                .validate_contract(&ResolvedProtocolContract::strict_anthropic_messages())
                .is_err()
        );
    }

    #[test]
    fn binding_debug_is_value_free() {
        let binding = ValidatedProtocolBinding::openai_chat();
        let debug = format!("{binding:?}");
        assert!(debug.contains("openai-chat-completions"));
        assert!(debug.contains("OpenAiChatCompletions"));
        assert!(debug.contains("openai-chat"));
        assert!(!debug.contains("CompatProfile"));
        assert!(!debug.contains("PolicySource"));
    }
}
