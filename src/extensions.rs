//! Protocol-scoped options that intentionally remain outside the common domain.

use std::fmt;

use serde_json::{Map, Value};

use crate::error::{ValidationError, ValidationReason};

const MAX_RAW_BYTES: usize = 64 * 1024;
const MAX_RAW_KEYS: usize = 128;
const MAX_RAW_ARRAY_ITEMS: usize = 1024;
const MAX_RAW_DEPTH: usize = 16;
const MAX_RAW_KEY_BYTES: usize = 64;

/// Stable protocol identifier for Anthropic Messages options.
pub const ANTHROPIC_MESSAGES_PROTOCOL_ID: &str = "anthropic-messages";

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

/// Value-free diagnostic emitted by protocol-scoped options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtocolOptionDiagnostic {
    /// A dangerous, non-portable raw body extension is active.
    NonPortableExtensionUsed,
}

/// Explicitly dangerous, bounded Anthropic body extension.
///
/// This is not a free-form request. It only admits unknown top-level body fields;
/// core request fields and every header/auth/version owner remain protected.
#[derive(Clone, Eq, PartialEq)]
pub struct AnthropicRawExtension {
    fields: Map<String, Value>,
    encoded_bytes: usize,
}

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
        let fields = value.as_object().ok_or_else(|| {
            raw_error(
                ValidationReason::Conflict,
                "raw extension must be a top-level JSON object",
            )
        })?;
        if fields.is_empty() {
            return Err(raw_error(
                ValidationReason::Empty,
                "raw extension object must not be empty",
            ));
        }
        let mut budget = RawBudget::default();
        validate_raw_value(&value, 1, &mut budget)?;
        let encoded_bytes = serde_json::to_vec(&value)
            .map_err(|_| {
                raw_error(
                    ValidationReason::Conflict,
                    "raw extension is not serializable",
                )
            })?
            .len();
        if encoded_bytes > MAX_RAW_BYTES {
            return Err(raw_error(
                ValidationReason::OutOfRange,
                "raw extension exceeds the encoded byte limit",
            ));
        }
        let Value::Object(fields) = value else {
            unreachable!("object shape was validated above")
        };
        Ok(Self {
            fields,
            encoded_bytes,
        })
    }

    /// Returns the value-free diagnostic associated with using this extension.
    #[must_use]
    pub const fn diagnostic(&self) -> ProtocolOptionDiagnostic {
        ProtocolOptionDiagnostic::NonPortableExtensionUsed
    }

    pub(crate) fn fields(&self) -> &Map<String, Value> {
        &self.fields
    }
}

impl fmt::Debug for AnthropicRawExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicRawExtension")
            .field("field_count", &self.fields.len())
            .field("encoded_bytes", &self.encoded_bytes)
            .finish()
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

/// Closed protocol-keyed option container.
#[derive(Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProtocolOptions {
    /// Anthropic Messages-only options.
    AnthropicMessages(AnthropicMessagesOptions),
}

impl ProtocolOptions {
    /// Returns the protocol identifier required by these options.
    #[must_use]
    pub const fn protocol_id(&self) -> &'static str {
        match self {
            Self::AnthropicMessages(_) => ANTHROPIC_MESSAGES_PROTOCOL_ID,
        }
    }

    /// Returns Anthropic options when this is the Anthropic variant.
    #[must_use]
    pub const fn anthropic_messages(&self) -> Option<&AnthropicMessagesOptions> {
        match self {
            Self::AnthropicMessages(options) => Some(options),
        }
    }

    /// Returns value-free option diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<ProtocolOptionDiagnostic> {
        match self {
            Self::AnthropicMessages(options) => options.diagnostics(),
        }
    }
}

impl From<AnthropicMessagesOptions> for ProtocolOptions {
    fn from(value: AnthropicMessagesOptions) -> Self {
        Self::AnthropicMessages(value)
    }
}

impl fmt::Debug for ProtocolOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnthropicMessages(options) => formatter
                .debug_tuple("AnthropicMessages")
                .field(options)
                .finish(),
        }
    }
}

#[derive(Default)]
struct RawBudget {
    keys: usize,
    array_items: usize,
}

fn validate_raw_value(
    value: &Value,
    depth: usize,
    budget: &mut RawBudget,
) -> Result<(), ValidationError> {
    if depth > MAX_RAW_DEPTH {
        return Err(raw_error(
            ValidationReason::OutOfRange,
            "raw extension exceeds the depth limit",
        ));
    }
    match value {
        Value::Object(fields) => {
            budget.keys = budget.keys.saturating_add(fields.len());
            if budget.keys > MAX_RAW_KEYS {
                return Err(raw_error(
                    ValidationReason::OutOfRange,
                    "raw extension exceeds the key limit",
                ));
            }
            for (key, child) in fields {
                if key.is_empty()
                    || key.len() > MAX_RAW_KEY_BYTES
                    || !key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(raw_error(
                        ValidationReason::InvalidIdentifier,
                        "raw extension contains a forbidden key shape",
                    ));
                }
                if depth == 1 && is_protected_top_level(key) {
                    return Err(raw_error(
                        ValidationReason::Conflict,
                        "raw extension attempts to override an SDK-owned field",
                    ));
                }
                validate_raw_value(child, depth + 1, budget)?;
            }
        }
        Value::Array(items) => {
            budget.array_items = budget.array_items.saturating_add(items.len());
            if budget.array_items > MAX_RAW_ARRAY_ITEMS {
                return Err(raw_error(
                    ValidationReason::OutOfRange,
                    "raw extension exceeds the array-item limit",
                ));
            }
            for item in items {
                validate_raw_value(item, depth + 1, budget)?;
            }
        }
        Value::String(value) if value.len() > MAX_RAW_BYTES => {
            return Err(raw_error(
                ValidationReason::OutOfRange,
                "raw extension contains an oversized string",
            ));
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn is_protected_top_level(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "model"
            | "messages"
            | "system"
            | "max_tokens"
            | "stream"
            | "temperature"
            | "tools"
            | "tool_choice"
            | "thinking"
            | "output_config"
            | "x-api-key"
            | "anthropic-version"
            | "anthropic-beta"
            | "content-type"
            | "accept"
            | "authorization"
            | "host"
            | "content-length"
            | "headers"
            | "header"
            | "auth"
            | "api_key"
            | "version"
            | "beta"
    )
}

fn raw_error(reason: ValidationReason, summary: &'static str) -> ValidationError {
    ValidationError::new("protocol_options.anthropic.raw", reason, summary)
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
    }

    #[test]
    fn raw_enforces_depth_key_array_and_byte_budgets() {
        let mut deep = Value::Bool(true);
        for _ in 0..MAX_RAW_DEPTH {
            deep = serde_json::json!({"nested": deep});
        }
        assert!(AnthropicRawExtension::dangerous_from_object(deep).is_err());

        let many_keys = Value::Object(
            (0..=MAX_RAW_KEYS)
                .map(|index| (format!("key_{index}"), Value::Null))
                .collect(),
        );
        assert!(AnthropicRawExtension::dangerous_from_object(many_keys).is_err());
        assert!(
            AnthropicRawExtension::dangerous_from_object(
                serde_json::json!({"items": vec![0; MAX_RAW_ARRAY_ITEMS + 1]})
            )
            .is_err()
        );
        assert!(
            AnthropicRawExtension::dangerous_from_object(
                serde_json::json!({"future": "x".repeat(MAX_RAW_BYTES + 1)})
            )
            .is_err()
        );
    }
}
