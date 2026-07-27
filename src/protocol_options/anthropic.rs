//! Anthropic Messages typed options and the bounded dangerous body extension.

use std::fmt;

use serde_json::{Map, Value};

use crate::error::ValidationError;
use crate::protected;

use super::ProtocolOptionDiagnostic;
use super::raw::RawFields;

const RAW_FIELD: &str = "protocol_options.anthropic.raw";

/// Anthropic adaptive-thinking display behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AnthropicThinkingDisplay {
    /// Omit visible thinking while retaining protocol-required signatures.
    Omitted,
    /// Ask Anthropic to return a summarized thinking block.
    Summarized,
}

/// Anthropic adaptive-thinking effort.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AnthropicEffort {
    /// Minimize reasoning effort.
    Low,
    /// Use medium reasoning effort.
    Medium,
    /// Use high reasoning effort.
    High,
    /// Use the maximum supported reasoning effort.
    Max,
}

/// Explicitly dangerous, bounded Anthropic body extension.
///
/// This is not a free-form request. It only admits unknown top-level body fields;
/// core request fields and every header/auth/version owner remain protected by
/// [`crate::protected`].
#[derive(Clone, Eq, PartialEq)]
pub struct AnthropicRawExtension(RawFields);

impl AnthropicRawExtension {
    /// Creates a dangerous raw extension after enforcing shape and resource limits.
    ///
    /// The explicit `dangerous` name is intentional: raw fields are not portable and
    /// receive no compatibility guarantee.
    ///
    /// # Errors
    ///
    /// Returns a value-free validation error when the value is not a non-empty object,
    /// contains a protected field/key shape, or exceeds a raw extension budget.
    pub fn dangerous_from_object(value: Value) -> Result<Self, ValidationError> {
        RawFields::parse(
            value,
            protected::ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS,
            RAW_FIELD,
        )
        .map(Self)
    }

    /// Returns the value-free diagnostic associated with using this extension.
    #[must_use]
    pub const fn diagnostic(&self) -> ProtocolOptionDiagnostic {
        ProtocolOptionDiagnostic::NonPortableExtensionUsed
    }

    pub(crate) const fn fields(&self) -> &Map<String, Value> {
        self.0.fields()
    }
}

impl fmt::Debug for AnthropicRawExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.debug("AnthropicRawExtension", formatter)
    }
}

/// Typed Anthropic Messages request options.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct AnthropicMessagesOptions {
    adaptive_thinking: Option<AnthropicThinkingDisplay>,
    effort: Option<AnthropicEffort>,
    raw: Option<AnthropicRawExtension>,
}

impl AnthropicMessagesOptions {
    /// Creates empty Anthropic Messages options.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            adaptive_thinking: None,
            effort: None,
            raw: None,
        }
    }

    /// Enables adaptive thinking with explicit display behavior.
    #[must_use]
    pub const fn with_adaptive_thinking(mut self, display: AnthropicThinkingDisplay) -> Self {
        self.adaptive_thinking = Some(display);
        self
    }

    /// Selects adaptive-thinking effort.
    #[must_use]
    pub const fn with_effort(mut self, effort: AnthropicEffort) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Adds a dangerous, bounded raw body extension.
    #[must_use]
    pub fn with_raw_extension(mut self, raw: AnthropicRawExtension) -> Self {
        self.raw = Some(raw);
        self
    }

    /// Returns adaptive-thinking display behavior when enabled.
    #[must_use]
    pub const fn adaptive_thinking(&self) -> Option<AnthropicThinkingDisplay> {
        self.adaptive_thinking
    }

    /// Returns adaptive-thinking effort when selected.
    #[must_use]
    pub const fn effort(&self) -> Option<AnthropicEffort> {
        self.effort
    }

    /// Returns value-free option diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<ProtocolOptionDiagnostic> {
        self.raw
            .as_ref()
            .map(AnthropicRawExtension::diagnostic)
            .into_iter()
            .collect()
    }

    pub(crate) const fn raw(&self) -> Option<&AnthropicRawExtension> {
        self.raw.as_ref()
    }
}

impl fmt::Debug for AnthropicMessagesOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicMessagesOptions")
            .field("adaptive_thinking", &self.adaptive_thinking)
            .field("effort", &self.effort)
            .field("raw", &self.raw)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_rejects_core_and_header_owners_without_leaking_values() {
        for value in [
            serde_json::json!({"model": "secret-canary"}),
            serde_json::json!({"anthropic-version": "secret-canary"}),
            serde_json::json!({"x-api-key": "secret-canary"}),
        ] {
            let error = AnthropicRawExtension::dangerous_from_object(value).unwrap_err();
            assert!(!error.to_string().contains("secret-canary"));
        }
    }

    #[test]
    fn raw_debug_and_diagnostics_are_value_free() {
        let raw = AnthropicRawExtension::dangerous_from_object(
            serde_json::json!({"future_feature": "secret-canary"}),
        )
        .unwrap();
        assert!(!format!("{raw:?}").contains("secret-canary"));
        assert_eq!(
            raw.diagnostic(),
            ProtocolOptionDiagnostic::NonPortableExtensionUsed
        );
        let options = AnthropicMessagesOptions::new().with_raw_extension(raw);
        assert_eq!(
            options.diagnostics(),
            vec![ProtocolOptionDiagnostic::NonPortableExtensionUsed]
        );
        assert!(!format!("{options:?}").contains("secret-canary"));
    }
}
