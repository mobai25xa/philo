//! Controlled JSON Schema compilation for tools and structured output.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::{SchemaError, SchemaFailure};

/// Official phase-two schema resource limits used by local validators.
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
    /// Official `OpenAI` profile defaults frozen for phase two.
    pub const fn official() -> Self {
        Self {
            max_schema_bytes: 256 * 1024,
            max_schema_depth: 32,
            max_json_array_items: 65_536,
        }
    }
}

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

/// A provider-independent tool or response schema with controlled validation metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolSchema {
    value: Value,
    strict_compatible: bool,
}

impl ToolSchema {
    /// Compiles and validates a local schema subset. No network access is performed.
    pub fn new(value: Value) -> Result<Self, SchemaError> {
        Self::with_limits(value, SchemaLimits::official())
    }

    /// Compiles a schema under the provided limits.
    pub fn with_limits(value: Value, limits: SchemaLimits) -> Result<Self, SchemaError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| {
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
        if json_depth(&value) > limits.max_schema_depth {
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

        let defs = collect_defs(&value)?;
        validate_schema_node(&value, "#", &defs, limits)?;
        let strict_compatible = is_strict_compatible_root(&value);
        Ok(Self {
            value,
            strict_compatible,
        })
    }

    /// Returns the preserved schema value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Returns whether the schema satisfies the local strict-mode object rules.
    pub fn is_strict_compatible(&self) -> bool {
        self.strict_compatible
    }
}

impl std::fmt::Debug for ToolSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSchema")
            .field("value_kind", &value_kind(&self.value))
            .field("strict_compatible", &self.strict_compatible)
            .finish_non_exhaustive()
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn collect_defs(value: &Value) -> Result<BTreeSet<String>, SchemaError> {
    let mut defs = BTreeSet::new();
    collect_defs_at(value, "#", &mut defs)?;
    Ok(defs)
}

fn collect_defs_at(
    value: &Value,
    path: &str,
    defs: &mut BTreeSet<String>,
) -> Result<(), SchemaError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
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
            let key = format!("#/$defs/{name}");
            if !defs.insert(key.clone()) {
                return Err(SchemaError::new(
                    "schema.$defs",
                    SchemaFailure::InvalidKeywordType,
                    Some(key),
                    "duplicate local definition name",
                ));
            }
            collect_defs_at(definition, &format!("{path}/$defs/{name}"), defs)?;
        }
    }
    for (key, child) in object {
        if key != "$defs" {
            collect_defs_at(child, &format!("{path}/{key}"), defs)?;
        }
    }
    Ok(())
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
        if !defs.contains(reference) {
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
            "additionalProperties must be a boolean in phase two",
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
    for key in [
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
    ] {
        if let Some(value) = object.get(key)
            && !value.is_number()
        {
            return Err(SchemaError::new(
                format!("schema.{key}"),
                SchemaFailure::InvalidKeywordType,
                Some(format!("{path}/{key}")),
                "numeric boundary keywords must be numbers",
            ));
        }
    }
    if let Some(Value::Number(max_items)) = object.get("maxItems")
        && let Some(max_items) = max_items.as_u64()
    {
        let max_items = usize::try_from(max_items).unwrap_or(usize::MAX);
        if max_items > limits.max_json_array_items {
            return Err(SchemaError::new(
                "schema.maxItems",
                SchemaFailure::TooLarge,
                Some(format!("{path}/maxItems")),
                "maxItems exceeds the allowed array length",
            ));
        }
    }
    Ok(())
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
