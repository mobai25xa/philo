//! Schema compilation and supported-keyword validation.

use std::collections::BTreeSet;

use serde_json::Value;

use super::budget::{SchemaLimits, json_depth};
use super::reference::{canonicalize_local_ref, collect_defs, validate_local_reference_graph};
use crate::error::{SchemaError, SchemaFailure};

const ALLOWED_KEYWORDS: &[&str] = &[
    "$schema",
    "$defs",
    "$ref",
    "type",
    "description",
    "title",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "anyOf",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "minItems",
    "maxItems",
];

pub(super) struct CompiledSchemaMetadata {
    pub(super) strict_compatible: bool,
}

pub(super) fn compile_schema(
    value: &Value,
    limits: SchemaLimits,
) -> Result<CompiledSchemaMetadata, SchemaError> {
    let encoded = serde_json::to_vec(value).map_err(|_| {
        SchemaError::new(
            "schema",
            SchemaFailure::InvalidKeywordType,
            None,
            "schema must be serializable JSON",
        )
    })?;
    if encoded.len() > limits.max_schema_bytes {
        return Err(SchemaError::new(
            "schema",
            SchemaFailure::TooLarge,
            None,
            "schema exceeds the allowed byte limit",
        ));
    }
    if json_depth(value) > limits.max_schema_depth {
        return Err(SchemaError::new(
            "schema",
            SchemaFailure::TooDeep,
            None,
            "schema exceeds the allowed nesting depth",
        ));
    }
    if !value.is_object() {
        return Err(SchemaError::new(
            "schema",
            SchemaFailure::NotAnObject,
            Some("#".to_owned()),
            "schema root must be a JSON object",
        ));
    }

    let defs = collect_defs(value)?;
    validate_schema_node(value, "#", &defs, limits)?;
    validate_local_reference_graph(value, &defs, limits)?;
    Ok(CompiledSchemaMetadata {
        strict_compatible: is_strict_compatible_root(value),
    })
}

pub(super) fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn validate_schema_node(
    value: &Value,
    path: &str,
    defs: &BTreeSet<String>,
    limits: SchemaLimits,
) -> Result<(), SchemaError> {
    let object = value.as_object().ok_or_else(|| {
        SchemaError::new(
            "schema",
            SchemaFailure::NotAnObject,
            Some(path.to_owned()),
            "schema nodes must be objects",
        )
    })?;

    for key in object.keys() {
        if !ALLOWED_KEYWORDS.contains(&key.as_str()) {
            return Err(SchemaError::new(
                "schema",
                SchemaFailure::UnsupportedKeyword,
                Some(format!("{path}/{key}")),
                "schema contains an unsupported keyword",
            ));
        }
    }

    if let Some(reference) = object.get("$ref") {
        let reference = reference.as_str().ok_or_else(|| {
            SchemaError::new(
                "schema.$ref",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/$ref")),
                "$ref must be a string",
            )
        })?;
        if !reference.starts_with("#/") {
            return Err(SchemaError::new(
                "schema.$ref",
                SchemaFailure::RemoteReference,
                Some(format!("{path}/$ref")),
                "remote schema references are not allowed",
            ));
        }
        let canonical = canonicalize_local_ref(reference).map_err(|()| {
            SchemaError::new(
                "schema.$ref",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/$ref")),
                "local schema reference contains an invalid JSON Pointer escape",
            )
        })?;
        if !defs.contains(&canonical) {
            return Err(SchemaError::new(
                "schema.$ref",
                SchemaFailure::UnresolvedLocalReference,
                Some(format!("{path}/$ref")),
                "local schema reference could not be resolved",
            ));
        }
    }

    if let Some(type_value) = object.get("type") {
        validate_type_keyword(type_value, path)?;
    }

    if let Some(required) = object.get("required") {
        let names = required.as_array().ok_or_else(|| {
            SchemaError::new(
                "schema.required",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/required")),
                "required must be an array of strings",
            )
        })?;
        let mut seen = BTreeSet::new();
        for (index, item) in names.iter().enumerate() {
            let name = item.as_str().ok_or_else(|| {
                SchemaError::new(
                    "schema.required",
                    SchemaFailure::InvalidKeywordType,
                    Some(format!("{path}/required/{index}")),
                    "required entries must be strings",
                )
            })?;
            if !seen.insert(name) {
                return Err(SchemaError::new(
                    "schema.required",
                    SchemaFailure::InvalidKeywordType,
                    Some(format!("{path}/required/{index}")),
                    "required entries must be unique",
                ));
            }
        }
        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            for name in &seen {
                if !properties.contains_key(*name) {
                    return Err(SchemaError::new(
                        "schema.required",
                        SchemaFailure::InvalidKeywordType,
                        Some(format!("{path}/required")),
                        "required names must exist in properties",
                    ));
                }
            }
        } else if !seen.is_empty() {
            return Err(SchemaError::new(
                "schema.required",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/required")),
                "required requires object properties",
            ));
        }
    }

    if let Some(additional) = object.get("additionalProperties")
        && !additional.is_boolean()
    {
        return Err(SchemaError::new(
            "schema.additionalProperties",
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/additionalProperties")),
            "additionalProperties must be a boolean",
        ));
    }

    if let Some(properties) = object.get("properties") {
        let map = properties.as_object().ok_or_else(|| {
            SchemaError::new(
                "schema.properties",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/properties")),
                "properties must be an object",
            )
        })?;
        for (name, child) in map {
            validate_schema_node(child, &format!("{path}/properties/{name}"), defs, limits)?;
        }
    }

    if let Some(items) = object.get("items") {
        validate_schema_node(items, &format!("{path}/items"), defs, limits)?;
    }

    if let Some(any_of) = object.get("anyOf") {
        let branches = any_of.as_array().ok_or_else(|| {
            SchemaError::new(
                "schema.anyOf",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/anyOf")),
                "anyOf must be an array",
            )
        })?;
        if branches.is_empty() {
            return Err(SchemaError::new(
                "schema.anyOf",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/anyOf")),
                "anyOf must not be empty",
            ));
        }
        for (index, branch) in branches.iter().enumerate() {
            validate_schema_node(branch, &format!("{path}/anyOf/{index}"), defs, limits)?;
        }
    }

    if let Some(definitions) = object.get("$defs") {
        let map = definitions.as_object().ok_or_else(|| {
            SchemaError::new(
                "schema.$defs",
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/$defs")),
                "$defs must be an object",
            )
        })?;
        for (name, definition) in map {
            validate_schema_node(definition, &format!("{path}/$defs/{name}"), defs, limits)?;
        }
    }

    validate_boundary_keywords(object, path, limits)
}

fn validate_type_keyword(type_value: &Value, path: &str) -> Result<(), SchemaError> {
    match type_value {
        Value::String(kind) => validate_basic_type(kind, path),
        Value::Array(kinds) => {
            if kinds.is_empty() {
                return Err(SchemaError::new(
                    "schema.type",
                    SchemaFailure::InvalidKeywordType,
                    Some(format!("{path}/type")),
                    "type arrays must not be empty",
                ));
            }
            let mut seen = BTreeSet::new();
            for (index, kind) in kinds.iter().enumerate() {
                let kind = kind.as_str().ok_or_else(|| {
                    SchemaError::new(
                        "schema.type",
                        SchemaFailure::InvalidKeywordType,
                        Some(format!("{path}/type/{index}")),
                        "type array entries must be strings",
                    )
                })?;
                if !seen.insert(kind) {
                    return Err(SchemaError::new(
                        "schema.type",
                        SchemaFailure::InvalidKeywordType,
                        Some(format!("{path}/type/{index}")),
                        "type array entries must be unique",
                    ));
                }
                validate_basic_type(kind, path)?;
            }
            Ok(())
        }
        _ => Err(SchemaError::new(
            "schema.type",
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/type")),
            "type must be a string or array of strings",
        )),
    }
}

fn validate_basic_type(kind: &str, path: &str) -> Result<(), SchemaError> {
    match kind {
        "object" | "array" | "string" | "number" | "integer" | "boolean" | "null" => Ok(()),
        _ => Err(SchemaError::new(
            "schema.type",
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/type")),
            "type uses an unsupported primitive",
        )),
    }
}

fn validate_boundary_keywords(
    object: &serde_json::Map<String, Value>,
    path: &str,
    limits: SchemaLimits,
) -> Result<(), SchemaError> {
    let min_length = optional_usize_keyword(object, "minLength", path)?;
    let max_length = optional_usize_keyword(object, "maxLength", path)?;
    let min_items = optional_usize_keyword(object, "minItems", path)?;
    let max_items = optional_usize_keyword(object, "maxItems", path)?;
    validate_ordered_usize_pair(min_length, max_length, "minLength", "maxLength", path)?;
    validate_ordered_usize_pair(min_items, max_items, "minItems", "maxItems", path)?;

    if max_items.is_some_and(|value| value > limits.max_json_array_items) {
        return Err(SchemaError::new(
            "schema.maxItems",
            SchemaFailure::TooLarge,
            Some(format!("{path}/maxItems")),
            "maxItems exceeds the allowed array length",
        ));
    }

    let minimum = optional_json_number(object, "minimum", path)?;
    let maximum = optional_json_number(object, "maximum", path)?;
    if matches!((minimum, maximum), (Some(min), Some(max)) if min > max) {
        return Err(SchemaError::new(
            "schema.minimum",
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/minimum")),
            "minimum must not exceed maximum",
        ));
    }
    Ok(())
}

fn optional_usize_keyword(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<usize>, SchemaError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let Some(raw) = value.as_u64() else {
        return Err(SchemaError::new(
            format!("schema.{key}"),
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/{key}")),
            "length and item boundaries must be non-negative integers",
        ));
    };
    let converted = usize::try_from(raw).map_err(|_| {
        SchemaError::new(
            format!("schema.{key}"),
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/{key}")),
            "length or item boundary cannot be represented by this SDK",
        )
    })?;
    Ok(Some(converted))
}

fn validate_ordered_usize_pair(
    minimum: Option<usize>,
    maximum: Option<usize>,
    minimum_key: &str,
    _maximum_key: &str,
    path: &str,
) -> Result<(), SchemaError> {
    if matches!((minimum, maximum), (Some(min), Some(max)) if min > max) {
        return Err(SchemaError::new(
            format!("schema.{minimum_key}"),
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/{minimum_key}")),
            "minimum boundary must not exceed maximum boundary",
        ));
    }
    Ok(())
}

fn optional_json_number(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<f64>, SchemaError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let Some(number) = value.as_f64().filter(|number| number.is_finite()) else {
        return Err(SchemaError::new(
            format!("schema.{key}"),
            SchemaFailure::InvalidKeywordType,
            Some(format!("{path}/{key}")),
            "numeric boundaries must be finite JSON numbers",
        ));
    };
    Ok(Some(number))
}

fn is_strict_compatible_root(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }
    object_is_strict(object)
}

fn object_is_strict(object: &serde_json::Map<String, Value>) -> bool {
    if object.get("additionalProperties") != Some(&Value::Bool(false)) {
        return false;
    }
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if properties
        .keys()
        .any(|name| !required.contains(name.as_str()))
    {
        return false;
    }
    properties.values().all(node_is_strict_compatible)
}

fn node_is_strict_compatible(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if let Some(branches) = object.get("anyOf").and_then(Value::as_array) {
        return !branches.is_empty() && branches.iter().all(node_is_strict_compatible);
    }
    match object.get("type") {
        Some(Value::String(kind)) if kind == "object" => object_is_strict(object),
        Some(Value::String(kind)) if kind == "array" => {
            object.get("items").is_some_and(node_is_strict_compatible)
        }
        Some(Value::String(_) | Value::Array(_)) | None => true,
        _ => false,
    }
}
