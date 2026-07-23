//! Complete JSON-instance validation against a compiled tool schema.

use serde_json::Value;

use super::budget::{SchemaLimits, preflight_instance};
use super::reference::{ReferenceGuard, canonicalize_local_ref, resolve_local_ref};
use crate::error::{ToolValidationError, ToolValidationFailure};

pub(super) fn validate_instance(
    schema: &Value,
    instance: &Value,
    limits: SchemaLimits,
) -> Result<(), ToolValidationError> {
    preflight_instance(instance, limits)?;
    let mut guard = ReferenceGuard::new(limits);
    validate_value(schema, instance, "#", schema, limits, &mut guard)
}

fn validate_value(
    schema: &Value,
    instance: &Value,
    path: &str,
    root: &Value,
    limits: SchemaLimits,
    guard: &mut ReferenceGuard,
) -> Result<(), ToolValidationError> {
    let Some(object) = schema.as_object() else {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::SchemaViolation,
            Some(path.to_owned()),
            "schema node is not an object",
        ));
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let canonical = canonicalize_local_ref(reference).map_err(|()| {
            ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "local schema reference contains an invalid JSON Pointer escape",
            )
        })?;
        let resolved = resolve_local_ref(root, &canonical).ok_or_else(|| {
            ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "local schema reference could not be resolved",
            )
        })?;
        guard.enter(&canonical, path)?;
        let result = validate_value(resolved, instance, path, root, limits, guard);
        guard.leave(&canonical);
        return result;
    }

    if let Some(any_of) = object.get("anyOf").and_then(Value::as_array) {
        let mut matched = false;
        for branch in any_of {
            match validate_value(branch, instance, path, root, limits, guard) {
                Ok(()) => {
                    matched = true;
                    break;
                }
                Err(error) if error.reason() == ToolValidationFailure::ArgumentsTooDeep => {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        if !matched {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "value matches no anyOf branch",
            ));
        }
    }

    if let Some(type_value) = object.get("type")
        && !matches_type(type_value, instance)
    {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::SchemaViolation,
            Some(path.to_owned()),
            "value does not match the declared type",
        ));
    }

    if let Some(constant) = object.get("const")
        && instance != constant
    {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::SchemaViolation,
            Some(path.to_owned()),
            "value does not equal const",
        ));
    }

    if let Some(enumeration) = object.get("enum").and_then(Value::as_array)
        && !enumeration.iter().any(|item| item == instance)
    {
        return Err(ToolValidationError::new(
            "arguments",
            ToolValidationFailure::SchemaViolation,
            Some(path.to_owned()),
            "value is not present in enum",
        ));
    }

    if let Some(text) = instance.as_str() {
        if let Some(min) = object.get("minLength").and_then(Value::as_u64)
            && (text.chars().count() as u64) < min
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "string is shorter than minLength",
            ));
        }
        if let Some(max) = object.get("maxLength").and_then(Value::as_u64)
            && (text.chars().count() as u64) > max
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "string is longer than maxLength",
            ));
        }
    }

    if let Some(number) = instance.as_f64() {
        if let Some(min) = object.get("minimum").and_then(Value::as_f64)
            && number < min
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "number is less than minimum",
            ));
        }
        if let Some(max) = object.get("maximum").and_then(Value::as_f64)
            && number > max
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "number is greater than maximum",
            ));
        }
    }

    if let Some(items) = instance.as_array() {
        if items.len() > limits.max_json_array_items {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::ArgumentsTooLarge,
                Some(path.to_owned()),
                "JSON array exceeds the allowed length",
            ));
        }
        if let Some(min) = object.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "array has fewer items than minItems",
            ));
        }
        if let Some(max) = object.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::SchemaViolation,
                Some(path.to_owned()),
                "array has more items than maxItems",
            ));
        }
        if let Some(item_schema) = object.get("items") {
            for (index, item) in items.iter().enumerate() {
                validate_value(
                    item_schema,
                    item,
                    &format!("{path}/{index}"),
                    root,
                    limits,
                    guard,
                )?;
            }
        }
    }

    if let Some(map) = instance.as_object() {
        let properties = object
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(required) = object.get("required").and_then(Value::as_array) {
            for (index, name) in required.iter().enumerate() {
                let Some(name) = name.as_str() else {
                    continue;
                };
                if !map.contains_key(name) {
                    return Err(ToolValidationError::new(
                        "arguments",
                        ToolValidationFailure::SchemaViolation,
                        Some(format!("{path}/required/{index}")),
                        "required property is missing",
                    ));
                }
            }
        }
        let additional = object
            .get("additionalProperties")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        for (key, child) in map {
            if let Some(property_schema) = properties.get(key) {
                validate_value(
                    property_schema,
                    child,
                    &format!("{path}/{key}"),
                    root,
                    limits,
                    guard,
                )?;
            } else if !additional {
                return Err(ToolValidationError::new(
                    "arguments",
                    ToolValidationFailure::SchemaViolation,
                    Some(format!("{path}/{key}")),
                    "additional property is not allowed",
                ));
            }
        }
    }

    Ok(())
}

fn matches_type(type_value: &Value, instance: &Value) -> bool {
    match type_value {
        Value::String(kind) => matches_basic_type(kind, instance),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .any(|kind| matches_basic_type(kind, instance)),
        _ => false,
    }
}

fn matches_basic_type(kind: &str, instance: &Value) -> bool {
    match kind {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "number" => instance.as_f64().is_some(),
        "integer" => {
            if instance.as_i64().is_some() || instance.as_u64().is_some() {
                true
            } else {
                instance
                    .as_f64()
                    .is_some_and(|value| value.fract() == 0.0 && value.is_finite())
            }
        }
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        _ => false,
    }
}
