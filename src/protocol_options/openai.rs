//! `OpenAI` Chat Completions typed options and the bounded dangerous body extension.

use std::fmt;

use serde_json::{Map, Value};

use crate::error::ValidationError;
use crate::protected;

use super::ProtocolOptionDiagnostic;
use super::raw::RawFields;

const RAW_FIELD: &str = "protocol_options.openai_chat.raw";

/// Explicitly dangerous, bounded `OpenAI` Chat Completions body extension.
///
/// This is the symmetric counterpart of
/// [`AnthropicRawExtension`](super::AnthropicRawExtension): same budgets, same key
/// shape rules, same value-free diagnostic. It only admits unknown top-level body
/// fields; core request fields and every header/auth/version owner remain protected
/// by [`crate::protected`].
///
/// Aggregation-gateway product parameters — upstream routing preferences, residency
/// constraints, retention constraints — are declared through this axis. They are
/// **not portable**: a body written for one gateway has no meaning on another, and
/// the SDK offers no compatibility guarantee for their contents.
#[derive(Clone, Eq, PartialEq)]
pub struct OpenAiChatRawExtension(RawFields);

impl OpenAiChatRawExtension {
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
            protected::OPENAI_CHAT_PROTECTED_BODY_FIELDS,
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

impl fmt::Debug for OpenAiChatRawExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.debug("OpenAiChatRawExtension", formatter)
    }
}

/// Typed `OpenAI` Chat Completions request options.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct OpenAiChatOptions {
    raw: Option<OpenAiChatRawExtension>,
}

impl OpenAiChatOptions {
    /// Creates empty `OpenAI` Chat Completions options.
    #[must_use]
    pub const fn new() -> Self {
        Self { raw: None }
    }

    /// Adds a dangerous, bounded raw body extension.
    #[must_use]
    pub fn with_raw_extension(mut self, raw: OpenAiChatRawExtension) -> Self {
        self.raw = Some(raw);
        self
    }

    /// Returns value-free option diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<ProtocolOptionDiagnostic> {
        self.raw
            .as_ref()
            .map(OpenAiChatRawExtension::diagnostic)
            .into_iter()
            .collect()
    }

    pub(crate) const fn raw(&self) -> Option<&OpenAiChatRawExtension> {
        self.raw.as_ref()
    }
}

impl fmt::Debug for OpenAiChatOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiChatOptions")
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
            serde_json::json!({"messages": "secret-canary"}),
            serde_json::json!({"stream_options": "secret-canary"}),
            serde_json::json!({"response_format": "secret-canary"}),
            serde_json::json!({"authorization": "secret-canary"}),
            serde_json::json!({"x-api-key": "secret-canary"}),
        ] {
            let error = OpenAiChatRawExtension::dangerous_from_object(value).unwrap_err();
            assert!(!error.to_string().contains("secret-canary"));
        }
    }

    #[test]
    fn gateway_routing_parameters_are_expressible_through_the_body_axis() {
        let raw = OpenAiChatRawExtension::dangerous_from_object(serde_json::json!({
            "provider": {
                "only": ["upstream-a"],
                "ignore": ["upstream-b"],
                "order": ["upstream-a"],
                "allow_fallbacks": false,
                "data_collection": "deny",
                "zdr": true,
                "sort": "throughput"
            }
        }))
        .unwrap();
        assert_eq!(
            raw.diagnostic(),
            ProtocolOptionDiagnostic::NonPortableExtensionUsed
        );
        assert_eq!(raw.fields().len(), 1);
    }

    #[test]
    fn raw_debug_and_diagnostics_are_value_free() {
        let raw = OpenAiChatRawExtension::dangerous_from_object(
            serde_json::json!({"future_feature": "secret-canary"}),
        )
        .unwrap();
        assert!(!format!("{raw:?}").contains("secret-canary"));
        let options = OpenAiChatOptions::new().with_raw_extension(raw);
        assert!(!format!("{options:?}").contains("secret-canary"));
        assert_eq!(
            options.diagnostics(),
            vec![ProtocolOptionDiagnostic::NonPortableExtensionUsed]
        );
        assert!(OpenAiChatOptions::new().diagnostics().is_empty());
    }
}
