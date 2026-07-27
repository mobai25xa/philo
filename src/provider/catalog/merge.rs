//! Deterministic catalog merge and exact-model resolution.

use std::collections::BTreeMap;

use crate::domain::{CapabilityStatus, ReasoningEffortSupport};
use crate::error::LlmError;

use super::entry::{ModelEntry, ModelKey};

/// Immutable catalog snapshot used by a provider runtime.
#[derive(Clone, Debug, Default)]
pub struct ModelCatalog {
    entries: BTreeMap<ModelKey, ModelEntry>,
}

impl ModelCatalog {
    /// Creates a catalog from validated exact entries.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when an entry is invalid or an exact key is duplicated.
    pub fn from_entries(entries: impl IntoIterator<Item = ModelEntry>) -> Result<Self, LlmError> {
        let mut catalog = Self::default();
        for entry in entries {
            catalog.insert(entry)?;
        }
        Ok(catalog)
    }

    /// Inserts an exact entry and rejects duplicates.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the entry is invalid or its exact key already exists.
    pub fn insert(&mut self, mut entry: ModelEntry) -> Result<(), LlmError> {
        entry.validate()?;
        entry.seed_provenance();
        if self.entries.contains_key(&entry.key) {
            return Err(LlmError::Configuration(
                "duplicate exact catalog key".to_owned(),
            ));
        }
        self.entries.insert(entry.key.clone(), entry);
        Ok(())
    }

    /// Returns an exact entry.
    #[must_use]
    pub fn get(&self, key: &ModelKey) -> Option<&ModelEntry> {
        self.entries.get(key)
    }

    /// Returns deterministic entries in key order.
    pub fn entries(&self) -> impl Iterator<Item = &ModelEntry> {
        self.entries.values()
    }

    /// Resolves an exact entry by provider/product/domain model.
    #[must_use]
    pub fn resolve(&self, key: &ModelKey) -> Option<&ModelEntry> {
        self.get(key)
    }

    /// Merges ordered catalogs. Later layers replace only explicitly known capability/limit facts.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when any merged entry violates catalog invariants.
    pub fn merge(layers: &[&ModelCatalog]) -> Result<Self, LlmError> {
        let mut result = Self::default();
        for layer in layers {
            for entry in layer.entries.values() {
                if let Some(previous) = result.entries.get_mut(&entry.key) {
                    merge_entry(previous, entry);
                    previous.validate()?;
                } else {
                    result.insert(entry.clone())?;
                }
            }
        }
        Ok(result)
    }
}

fn merge_entry(base: &mut ModelEntry, overlay: &ModelEntry) {
    base.provider_model_id = overlay.provider_model_id.clone();
    base.deployment_id = overlay
        .deployment_id
        .clone()
        .or_else(|| base.deployment_id.clone());
    base.wire_model_value = overlay.wire_model_value.clone();
    if !overlay.display_name.is_empty() {
        base.display_name.clone_from(&overlay.display_name);
    }
    base.protocol_id = overlay.protocol_id.clone();
    for field in [
        "provider_model_id",
        "wire_model_value",
        "display_name",
        "protocol_id",
    ] {
        copy_source(base, overlay, field);
    }
    if overlay.deployment_id.is_some() {
        copy_source(base, overlay, "deployment_id");
    }
    merge_capabilities(base, overlay);
    merge_limits(base, overlay);
    if overlay.default_max_output_tokens.is_some() {
        base.default_max_output_tokens = overlay.default_max_output_tokens;
        copy_source(base, overlay, "default_max_output_tokens");
    }
    if overlay.pricing.is_some() {
        base.pricing.clone_from(&overlay.pricing);
        copy_source(base, overlay, "pricing");
    }
    base.source = overlay.source.clone();
    if !matches!(overlay.support_status, CapabilityStatus::Unknown) {
        base.support_status = overlay.support_status;
        copy_source(base, overlay, "support_status");
    }
}

fn merge_capabilities(base: &mut ModelEntry, overlay: &ModelEntry) {
    macro_rules! status {
        ($field:ident) => {
            if !matches!(overlay.capabilities.$field, CapabilityStatus::Unknown) {
                base.capabilities.$field = overlay.capabilities.$field;
                copy_source(base, overlay, concat!("capabilities.", stringify!($field)));
            }
        };
    }
    status!(function_tools);
    status!(tool_choice_required);
    status!(tool_choice_specific);
    status!(parallel_tool_calls);
    status!(strict_tools);
    status!(vision_input);
    status!(image_detail_original);
    status!(response_format_json_object);
    status!(response_format_json_schema);
    if !matches!(
        overlay.capabilities.reasoning_efforts,
        ReasoningEffortSupport::Unknown
    ) {
        base.capabilities.reasoning_efforts = overlay.capabilities.reasoning_efforts.clone();
        copy_source(base, overlay, "capabilities.reasoning_efforts");
    }
    status!(adaptive_thinking);
    status!(adaptive_thinking_effort);
}

fn merge_limits(base: &mut ModelEntry, overlay: &ModelEntry) {
    macro_rules! limit {
        ($field:ident) => {
            if overlay.limits.$field.is_some() {
                base.limits.$field = overlay.limits.$field;
                copy_source(base, overlay, concat!("limits.", stringify!($field)));
            }
        };
    }
    limit!(context_window_tokens);
    limit!(max_output_tokens);
    limit!(max_messages);
    limit!(max_tools);
    limit!(max_images);
    limit!(max_schema_bytes);
}

fn copy_source(base: &mut ModelEntry, overlay: &ModelEntry, field: &str) {
    let source = overlay
        .field_source(field)
        .unwrap_or(&overlay.source)
        .clone();
    base.provenance.insert(field.to_owned(), source);
}
