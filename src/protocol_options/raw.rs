//! Shared, bounded raw body-extension core.
//!
//! Both protocol raw extensions are the same mechanism with a different protected
//! field table. The budget, key shape, depth, and value-free error reporting live
//! here exactly once.

use std::fmt;

use serde_json::{Map, Value};

use crate::error::{ValidationError, ValidationReason};
use crate::protected;

const MAX_RAW_BYTES: usize = 64 * 1024;
const MAX_RAW_KEYS: usize = 128;
const MAX_RAW_ARRAY_ITEMS: usize = 1024;
const MAX_RAW_DEPTH: usize = 16;
const MAX_RAW_KEY_BYTES: usize = 64;

/// Validated raw top-level body fields with their encoded size.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct RawFields {
    fields: Map<String, Value>,
    encoded_bytes: usize,
}

impl RawFields {
    /// Validates shape, protected keys, and resource budgets for one raw object.
    pub(crate) fn parse(
        value: Value,
        protocol_fields: &'static [&'static str],
        field: &'static str,
    ) -> Result<Self, ValidationError> {
        let fields = value.as_object().ok_or_else(|| {
            error(
                field,
                ValidationReason::Conflict,
                "raw extension must be a top-level JSON object",
            )
        })?;
        if fields.is_empty() {
            return Err(error(
                field,
                ValidationReason::Empty,
                "raw extension object must not be empty",
            ));
        }
        let mut budget = RawBudget::default();
        validate_raw_value(&value, 1, &mut budget, protocol_fields, field)?;
        let encoded_bytes = serde_json::to_vec(&value)
            .map_err(|_| {
                error(
                    field,
                    ValidationReason::Conflict,
                    "raw extension is not serializable",
                )
            })?
            .len();
        if encoded_bytes > MAX_RAW_BYTES {
            return Err(error(
                field,
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

    pub(crate) const fn fields(&self) -> &Map<String, Value> {
        &self.fields
    }

    pub(crate) fn debug(
        &self,
        name: &'static str,
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        formatter
            .debug_struct(name)
            .field("field_count", &self.fields.len())
            .field("encoded_bytes", &self.encoded_bytes)
            .finish()
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
    protocol_fields: &'static [&'static str],
    field: &'static str,
) -> Result<(), ValidationError> {
    if depth > MAX_RAW_DEPTH {
        return Err(error(
            field,
            ValidationReason::OutOfRange,
            "raw extension exceeds the depth limit",
        ));
    }
    match value {
        Value::Object(fields) => {
            budget.keys = budget.keys.saturating_add(fields.len());
            if budget.keys > MAX_RAW_KEYS {
                return Err(error(
                    field,
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
                    return Err(error(
                        field,
                        ValidationReason::InvalidIdentifier,
                        "raw extension contains a forbidden key shape",
                    ));
                }
                if depth == 1 && protected::is_protected_body_field(protocol_fields, key) {
                    return Err(error(
                        field,
                        ValidationReason::Conflict,
                        "raw extension attempts to override an SDK-owned field",
                    ));
                }
                validate_raw_value(child, depth + 1, budget, protocol_fields, field)?;
            }
        }
        Value::Array(items) => {
            budget.array_items = budget.array_items.saturating_add(items.len());
            if budget.array_items > MAX_RAW_ARRAY_ITEMS {
                return Err(error(
                    field,
                    ValidationReason::OutOfRange,
                    "raw extension exceeds the array-item limit",
                ));
            }
            for item in items {
                validate_raw_value(item, depth + 1, budget, protocol_fields, field)?;
            }
        }
        Value::String(value) if value.len() > MAX_RAW_BYTES => {
            return Err(error(
                field,
                ValidationReason::OutOfRange,
                "raw extension contains an oversized string",
            ));
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn error(field: &'static str, reason: ValidationReason, summary: &'static str) -> ValidationError {
    ValidationError::new(field, reason, summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD: &str = "protocol_options.test.raw";

    fn parse(value: Value) -> Result<RawFields, ValidationError> {
        RawFields::parse(value, protected::OPENAI_CHAT_PROTECTED_BODY_FIELDS, FIELD)
    }

    #[test]
    fn budgets_are_enforced_for_depth_keys_arrays_and_bytes() {
        let mut deep = Value::Bool(true);
        for _ in 0..MAX_RAW_DEPTH {
            deep = serde_json::json!({ "nested": deep });
        }
        assert!(parse(deep).is_err());

        let many_keys = Value::Object(
            (0..=MAX_RAW_KEYS)
                .map(|index| (format!("key_{index}"), Value::Null))
                .collect(),
        );
        assert!(parse(many_keys).is_err());
        assert!(parse(serde_json::json!({"items": vec![0; MAX_RAW_ARRAY_ITEMS + 1]})).is_err());
        assert!(parse(serde_json::json!({"future": "x".repeat(MAX_RAW_BYTES + 1)})).is_err());
    }

    #[test]
    fn protection_uses_the_single_owner_table() {
        for key in ["model", "messages", "stream_options", "x-api-key"] {
            let Err(error) = parse(serde_json::json!({ key: "canary-secret-value" })) else {
                panic!("protected key {key} was admitted");
            };
            assert!(!error.to_string().contains("canary-secret-value"));
        }
        assert!(parse(serde_json::json!({"provider": {"sort": "price"}})).is_ok());
    }
}
