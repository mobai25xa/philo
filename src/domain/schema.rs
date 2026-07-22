//! Controlled JSON Schema compilation for tools and structured output.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::limits::ResourceLimits;
use crate::error::{SchemaError, SchemaFailure, ToolValidationError, ToolValidationFailure};

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
        validate_local_reference_graph(&value, &defs, limits)?;
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

    /// Validates a complete JSON instance against this schema.
    ///
    /// Failures never include argument values or secret material. Only field
    /// paths and stable reason codes are retained.
    pub fn validate_instance(
        &self,
        instance: &Value,
        limits: SchemaLimits,
    ) -> Result<(), ToolValidationError> {
        preflight_instance(instance, limits)?;
        let root = &self.value;
        let mut guard = ReferenceGuard::new(limits);
        validate_value(root, instance, "#", root, limits, &mut guard)
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
            let key = format!("{path}/$defs/{}", encode_pointer_segment(name));
            if !defs.insert(key.clone()) {
                return Err(SchemaError::new(
                    "schema.$defs",
                    SchemaFailure::InvalidKeywordType,
                    Some(key),
                    "duplicate local definition name",
                ));
            }
            collect_defs_at(definition, &key, defs)?;
        }
    }
    for (key, child) in object {
        if key != "$defs" {
            collect_defs_at(child, &format!("{path}/{key}"), defs)?;
        }
    }
    Ok(())
}

fn validate_local_reference_graph(
    root: &Value,
    defs: &BTreeSet<String>,
    limits: SchemaLimits,
) -> Result<(), SchemaError> {
    let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
    let mut nodes = Vec::with_capacity(defs.len() + 1);
    nodes.push("#".to_owned());
    nodes.extend(defs.iter().cloned());
    for node in &nodes {
        let schema = if node == "#" {
            root
        } else {
            resolve_local_ref(root, node).ok_or_else(|| {
                SchemaError::new(
                    "schema.$ref",
                    SchemaFailure::UnresolvedLocalReference,
                    None,
                    "local schema definition could not be resolved",
                )
            })?
        };
        let mut references = BTreeSet::new();
        collect_semantic_references(schema, &mut references)?;
        graph.insert(node.clone(), references);
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut expansion_count = 0usize;
    let max_expansions = limits
        .max_schema_depth
        .max(1)
        .saturating_mul(nodes.len().max(1));
    for node in nodes {
        visit_reference_node(
            &node,
            &graph,
            &mut visiting,
            &mut visited,
            0,
            limits.max_schema_depth,
            &mut expansion_count,
            max_expansions,
        )?;
    }
    Ok(())
}

fn collect_semantic_references(
    value: &Value,
    references: &mut BTreeSet<String>,
) -> Result<(), SchemaError> {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref") {
                let reference = reference.as_str().ok_or_else(|| {
                    SchemaError::new(
                        "schema.$ref",
                        SchemaFailure::InvalidKeywordType,
                        None,
                        "$ref must be a string",
                    )
                })?;
                references.insert(canonicalize_local_ref(reference).map_err(|()| {
                    SchemaError::new(
                        "schema.$ref",
                        SchemaFailure::InvalidKeywordType,
                        None,
                        "local schema reference contains an invalid JSON Pointer escape",
                    )
                })?);
            }
            for (key, child) in object {
                if key != "$defs" {
                    collect_semantic_references(child, references)?;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_semantic_references(item, references)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn visit_reference_node(
    node: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    depth: usize,
    max_depth: usize,
    expansion_count: &mut usize,
    max_expansions: usize,
) -> Result<(), SchemaError> {
    if visited.contains(node) {
        return Ok(());
    }
    if depth > max_depth || !visiting.insert(node.to_owned()) {
        return Err(reference_too_deep());
    }
    *expansion_count = expansion_count.saturating_add(1);
    if *expansion_count > max_expansions {
        return Err(reference_too_deep());
    }
    if let Some(edges) = graph.get(node) {
        for edge in edges {
            visit_reference_node(
                edge,
                graph,
                visiting,
                visited,
                depth.saturating_add(1),
                max_depth,
                expansion_count,
                max_expansions,
            )?;
        }
    }
    visiting.remove(node);
    visited.insert(node.to_owned());
    Ok(())
}

fn reference_too_deep() -> SchemaError {
    SchemaError::new(
        "schema.$ref",
        SchemaFailure::TooDeep,
        None,
        "local schema references are cyclic or exceed the expansion budget",
    )
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

fn preflight_instance(instance: &Value, limits: SchemaLimits) -> Result<(), ToolValidationError> {
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

struct ReferenceGuard {
    active_refs: BTreeSet<String>,
    expansion_count: usize,
    max_expansions: usize,
}

impl ReferenceGuard {
    fn new(limits: SchemaLimits) -> Self {
        Self {
            active_refs: BTreeSet::new(),
            expansion_count: 0,
            max_expansions: limits
                .max_schema_depth
                .max(1)
                .saturating_mul(limits.max_schema_depth.max(1)),
        }
    }

    fn enter(&mut self, reference: &str, path: &str) -> Result<(), ToolValidationError> {
        self.expansion_count = self.expansion_count.saturating_add(1);
        if self.expansion_count > self.max_expansions
            || !self.active_refs.insert(reference.to_owned())
        {
            return Err(ToolValidationError::new(
                "arguments",
                ToolValidationFailure::ArgumentsTooDeep,
                Some(path.to_owned()),
                "local schema reference expansion exceeded the safety budget",
            ));
        }
        Ok(())
    }

    fn leave(&mut self, reference: &str) {
        self.active_refs.remove(reference);
    }
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

fn resolve_local_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if !reference.starts_with("#/") {
        return None;
    }
    let mut current = root;
    for segment in reference.strip_prefix("#/")?.split('/') {
        let segment = decode_pointer_segment(segment).ok()?;
        let object = current.as_object()?;
        current = object.get(segment.as_str())?;
    }
    Some(current)
}

fn canonicalize_local_ref(reference: &str) -> Result<String, ()> {
    let pointer = reference.strip_prefix("#/").ok_or(())?;
    let mut canonical = String::from("#");
    for raw in pointer.split('/') {
        let decoded = decode_pointer_segment(raw)?;
        canonical.push('/');
        canonical.push_str(&encode_pointer_segment(&decoded));
    }
    Ok(canonical)
}

fn decode_pointer_segment(segment: &str) -> Result<String, ()> {
    let mut decoded = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => return Err(()),
        }
    }
    Ok(decoded)
}

fn encode_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}
