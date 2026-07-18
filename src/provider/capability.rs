//! Provider capability, dialect, and transport safety declarations.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use crate::domain::{CapabilitySet, CapabilityStatus};
use crate::error::LlmError;
use crate::provider::endpoint::RedirectPolicy;

/// Date on which official phase-one capability declarations were last reviewed.
pub const OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE: &str = "2026-07-18";

/// Provider/model capabilities needed during phase one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}

impl ProviderCapabilities {
    /// Returns the subset used by domain request validation.
    pub fn generation_options(self) -> CapabilitySet {
        CapabilitySet {
            temperature: self.temperature,
            max_output_tokens: self.max_completion_tokens,
        }
    }

    pub(super) fn official_openai() -> Self {
        Self {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
        }
    }

    pub(super) fn validate(self) -> Result<(), LlmError> {
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
}

/// Protocol-specific response/request behavior selected by a profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolDialect {
    /// Official `OpenAI` Chat Completions semantics.
    OpenAiChatCompletions,
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
