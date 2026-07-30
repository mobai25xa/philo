//! Schema and instance resource-budget enforcement.

use serde_json::Value;

use super::super::limits::ResourceLimits;
use crate::error::{ToolValidationError, ToolValidationFailure};

/// Official schema resource limits used by local validators.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaLimits {
    /// Maximum UTF-8 byte size of one schema document.
    pub max_schema_bytes: usize,
    /// Maximum JSON nesting depth of one schema document.
    pub max_schema_depth: usize,
    /// Maximum number of items allowed while validating arrays later.
    pub max_json_array_items: usize,
}

impl SchemaLimits {
    /// Official `OpenAI` profile defaults.
    pub const fn official() -> Self {
        Self {
            max_schema_bytes: 256 * 1024,
            max_schema_depth: 32,
            max_json_array_items: 65_536,
        }
    }
}

pub(super) fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

pub(super) fn preflight_instance(
    instance: &Value,
    limits: SchemaLimits,
) -> Result<(), ToolValidationError> {
    let encoded = serde_json::to_vec(instance).map_err(|_| {
        ToolValidationError::new(
            "arguments",
            ToolValidationFailure::InvalidJson,
            Some("#".to_owned()),
            "arguments must be serializable JSON",
        )
    })?;
    if encoded.len() > ResourceLimits::official().max_tool_arguments_bytes {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::ArgumentsTooLarge,
            Some("#".to_owned()),
            "tool arguments exceed the allowed byte limit",
        ));
    }
    if json_depth(instance) > limits.max_schema_depth {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::ArgumentsTooDeep,
            Some("#".to_owned()),
            "tool arguments exceed the allowed nesting depth",
        ));
    }
    check_array_lengths(instance, "#", limits)
}

fn check_array_lengths(
    value: &Value,
    path: &str,
    limits: SchemaLimits,
) -> Result<(), ToolValidationError> {
    match value {
        Value::Array(items) => {
            if items.len() > limits.max_json_array_items {
                return Err(ToolValidationError::new(
                    "arguments",
                    ToolValidationFailure::ArgumentsTooLarge,
                    Some(path.to_owned()),
                    "JSON array exceeds the allowed length",
                ));
            }
            for (index, item) in items.iter().enumerate() {
                check_array_lengths(item, &format!("{path}/{index}"), limits)?;
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                check_array_lengths(child, &format!("{path}/{key}"), limits)?;
            }
        }
        _ => {}
    }
    Ok(())
}
