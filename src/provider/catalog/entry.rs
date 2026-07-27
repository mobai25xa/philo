//! Exact model catalog entries and constrained capability/limit metadata.

use std::collections::BTreeMap;

use crate::domain::{
    CapabilityStatus, ModelId, PriceProfile, ProtocolId, ProviderId, ReasoningEffortSupport,
    ResourceLimits,
};
use crate::error::LlmError;

use super::ids::{DeploymentId, ProductId, ProviderModelId, WireModelValue};
use super::source::CatalogSource;

/// Model-specific limits; absent fields remain unknown rather than zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelLimits {
    /// Context window in tokens, when officially documented.
    pub context_window_tokens: Option<u32>,
    /// Maximum output tokens.
    pub max_output_tokens: Option<u32>,
    /// Maximum history messages.
    pub max_messages: Option<usize>,
    /// Maximum tool definitions.
    pub max_tools: Option<usize>,
    /// Maximum images.
    pub max_images: Option<usize>,
    /// Maximum schema bytes.
    pub max_schema_bytes: Option<usize>,
}

impl ModelLimits {
    /// Validates positive limits.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when a declared limit is zero.
    pub fn validate(self) -> Result<(), LlmError> {
        for value in [
            self.context_window_tokens.map(u64::from),
            self.max_output_tokens.map(u64::from),
        ] {
            if value == Some(0) {
                return Err(LlmError::Configuration(
                    "catalog token limit must be positive".to_owned(),
                ));
            }
        }
        for value in [
            self.max_messages,
            self.max_tools,
            self.max_images,
            self.max_schema_bytes,
        ] {
            if value == Some(0) {
                return Err(LlmError::Configuration(
                    "catalog resource limit must be positive".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Applies model ceilings to the SDK's transport/resource snapshot.
    #[must_use]
    pub fn apply_to(self, mut base: ResourceLimits) -> ResourceLimits {
        if let Some(value) = self.max_messages {
            base.max_messages = base.max_messages.min(value);
        }
        if let Some(value) = self.max_tools {
            base.max_tools = base.max_tools.min(value);
        }
        if let Some(value) = self.max_images {
            base.max_images = base.max_images.min(value);
        }
        if let Some(value) = self.max_schema_bytes {
            base.max_schema_bytes = base.max_schema_bytes.min(value);
        }
        base
    }
}

/// Provider defaults that may be merged with exact model facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDefaults {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// Provider product identity.
    pub product_id: ProductId,
    /// Default capability values.
    pub capabilities: CatalogCapabilities,
    /// Default resource limits.
    pub limits: ModelLimits,
    /// Source evidence.
    pub source: CatalogSource,
}

/// Structured capability declaration stored by the catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogCapabilities {
    /// Function tools.
    pub function_tools: CapabilityStatus,
    /// Required tool choice.
    pub tool_choice_required: CapabilityStatus,
    /// Specific tool choice.
    pub tool_choice_specific: CapabilityStatus,
    /// Parallel tools.
    pub parallel_tool_calls: CapabilityStatus,
    /// Strict tools.
    pub strict_tools: CapabilityStatus,
    /// Vision input.
    pub vision_input: CapabilityStatus,
    /// Original image detail.
    pub image_detail_original: CapabilityStatus,
    /// JSON object output.
    pub response_format_json_object: CapabilityStatus,
    /// JSON schema output.
    pub response_format_json_schema: CapabilityStatus,
    /// Reasoning efforts.
    pub reasoning_efforts: ReasoningEffortSupport,
    /// Protocol-scoped adaptive-thinking request support.
    pub adaptive_thinking: CapabilityStatus,
    /// Protocol-scoped adaptive-thinking effort support.
    pub adaptive_thinking_effort: CapabilityStatus,
}

impl Default for CatalogCapabilities {
    fn default() -> Self {
        Self {
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
            adaptive_thinking: CapabilityStatus::Unknown,
            adaptive_thinking_effort: CapabilityStatus::Unknown,
        }
    }
}

/// Exact provider/product/domain model catalog key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelKey {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// Product identity.
    pub product_id: ProductId,
    /// Domain model identity.
    pub domain_model_id: ModelId,
}

/// One exact model/deployment/wire mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelEntry {
    /// Exact lookup key.
    pub key: ModelKey,
    /// Provider model identity.
    pub provider_model_id: ProviderModelId,
    /// Optional deployment identity.
    pub deployment_id: Option<DeploymentId>,
    /// Wire model value.
    pub wire_model_value: WireModelValue,
    /// Human-readable display name.
    pub display_name: String,
    /// Protocol adapter.
    pub protocol_id: ProtocolId,
    /// Exact capability facts.
    pub capabilities: CatalogCapabilities,
    /// Exact model limits.
    pub limits: ModelLimits,
    /// Default options that are safe to apply.
    pub default_max_output_tokens: Option<u32>,
    /// Optional explicit local pricing.
    pub pricing: Option<PriceProfile>,
    /// Evidence/source.
    pub source: CatalogSource,
    /// Three-state overall availability decision.
    ///
    /// Evidence maturity and freshness remain independent in [`Self::source`]
    /// and the checked support matrix; they never add decision variants here.
    pub support_status: CapabilityStatus,
    /// Field-level provenance labels.
    pub provenance: BTreeMap<String, CatalogSource>,
}

impl ModelEntry {
    /// Validates exact-key and limit invariants.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for incomplete keys, invalid limits, an empty
    /// display name, or a default output value above the exact model limit.
    pub fn validate(&self) -> Result<(), LlmError> {
        if self.key.provider_id.as_str().is_empty() || self.key.product_id.as_str().is_empty() {
            return Err(LlmError::Configuration(
                "catalog key is incomplete".to_owned(),
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(LlmError::Configuration(
                "catalog display name must not be empty".to_owned(),
            ));
        }
        self.limits.validate()?;
        if self.default_max_output_tokens == Some(0) {
            return Err(LlmError::Configuration(
                "catalog default output limit must be positive".to_owned(),
            ));
        }
        if let (Some(default), Some(maximum)) = (
            self.default_max_output_tokens,
            self.limits.max_output_tokens,
        ) && default > maximum
        {
            return Err(LlmError::Configuration(
                "catalog default output tokens exceed the exact model limit".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the evidence source for one stable field path.
    #[must_use]
    pub fn field_source(&self, field: &str) -> Option<&CatalogSource> {
        self.provenance.get(field)
    }

    pub(super) fn seed_provenance(&mut self) {
        for field in [
            "provider_model_id",
            "deployment_id",
            "wire_model_value",
            "display_name",
            "protocol_id",
            "capabilities.function_tools",
            "capabilities.tool_choice_required",
            "capabilities.tool_choice_specific",
            "capabilities.parallel_tool_calls",
            "capabilities.strict_tools",
            "capabilities.vision_input",
            "capabilities.image_detail_original",
            "capabilities.response_format_json_object",
            "capabilities.response_format_json_schema",
            "capabilities.reasoning_efforts",
            "capabilities.adaptive_thinking",
            "capabilities.adaptive_thinking_effort",
            "limits.context_window_tokens",
            "limits.max_output_tokens",
            "limits.max_messages",
            "limits.max_tools",
            "limits.max_images",
            "limits.max_schema_bytes",
            "default_max_output_tokens",
            "pricing",
            "support_status",
        ] {
            self.provenance
                .entry(field.to_owned())
                .or_insert_with(|| self.source.clone());
        }
    }
}
