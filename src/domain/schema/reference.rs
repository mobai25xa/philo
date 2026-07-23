//! Local JSON Pointer resolution and reference-expansion guards.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::SchemaLimits;
use crate::error::{SchemaError, SchemaFailure, ToolValidationError, ToolValidationFailure};

pub(super) fn collect_defs(value: &Value) -> Result<BTreeSet<String>, SchemaError> {
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

pub(super) fn validate_local_reference_graph(
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

pub(super) struct ReferenceGuard {
    active_refs: BTreeSet<String>,
    expansion_count: usize,
    max_expansions: usize,
}

impl ReferenceGuard {
    pub(super) fn new(limits: SchemaLimits) -> Self {
        Self {
            active_refs: BTreeSet::new(),
            expansion_count: 0,
            max_expansions: limits
                .max_schema_depth
                .max(1)
                .saturating_mul(limits.max_schema_depth.max(1)),
        }
    }

    pub(super) fn enter(&mut self, reference: &str, path: &str) -> Result<(), ToolValidationError> {
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

    pub(super) fn leave(&mut self, reference: &str) {
        self.active_refs.remove(reference);
    }
}

pub(super) fn resolve_local_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
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

pub(super) fn canonicalize_local_ref(reference: &str) -> Result<String, ()> {
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
