//! Provider-scoped typed routing for aggregation gateways.
//!
//! These types intentionally live outside the domain model. Only one currently
//! reviewed provider family uses this wire contract, so promoting the concepts
//! to provider-neutral generation intent would be premature.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::PolicySource;
use crate::error::{LlmError, ValidationError, ValidationReason};

/// Stable upstream identifier accepted by a provider routing contract.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UpstreamId(String);

impl UpstreamId {
    /// Creates a bounded, printable upstream identifier.
    ///
    /// # Errors
    ///
    /// Returns a validation error for empty, unbounded, non-ASCII, or unsupported input.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b' ')
            })
        {
            return Err(validation(
                "routing.upstream",
                ValidationReason::InvalidIdentifier,
                "upstream must be bounded printable ASCII",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated wire identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable region or residency identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RoutingRegion(String);

impl RoutingRegion {
    /// Creates a lowercase region identifier.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the identifier is not bounded lowercase ASCII.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.trim() == value
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            });
        if !valid {
            return Err(validation(
                "routing.region",
                ValidationReason::InvalidIdentifier,
                "region must be bounded lowercase ASCII",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether a routing constraint is mandatory or merely preferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConstraintStrength {
    /// Failure to satisfy the constraint must fail closed.
    Hard,
    /// The provider may use the value as a non-binding preference.
    Preferred,
}

/// Typed data-retention requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataRetention {
    /// No additional retention constraint.
    Allowed,
    /// Providers that collect request data must be excluded.
    Denied,
    /// Zero-data-retention processing is required.
    ZeroDataRetention,
}

impl DataRetention {
    const fn strictness(self) -> u8 {
        match self {
            Self::Allowed => 0,
            Self::Denied => 1,
            Self::ZeroDataRetention => 2,
        }
    }
}

/// Provider ordering preference supported by the reviewed gateway contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingSort {
    /// Prefer lower estimated cost.
    Price,
    /// Prefer lower observed latency.
    Latency,
    /// Prefer higher observed throughput.
    Throughput,
}

/// Soft dimensions that fallback may relax.
///
/// Upstream allowlists, region/residency, and retention are deliberately absent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FallbackDimension {
    /// Relax the price preference.
    Price,
    /// Relax the latency preference.
    Latency,
    /// Relax the throughput preference.
    Throughput,
}

/// Fallback behavior separated from the dimensions it may relax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingFallback {
    enabled: bool,
    relaxations: BTreeSet<FallbackDimension>,
}

impl RoutingFallback {
    /// Creates fallback behavior.
    #[must_use]
    pub fn new(enabled: bool, relaxations: impl IntoIterator<Item = FallbackDimension>) -> Self {
        Self {
            enabled,
            relaxations: relaxations.into_iter().collect(),
        }
    }

    /// Returns whether provider fallback is enabled.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the explicitly relaxable soft dimensions.
    #[must_use]
    pub fn relaxations(&self) -> &BTreeSet<FallbackDimension> {
        &self.relaxations
    }
}

/// Stable fields used by value-free routing provenance.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RoutingField {
    /// Allowed upstream set.
    AllowedUpstreams,
    /// Denied upstream set.
    DeniedUpstreams,
    /// Provider order.
    ProviderOrder,
    /// Region/residency requirement.
    Region,
    /// Data-retention requirement.
    DataRetention,
    /// Fallback behavior.
    Fallback,
    /// Provider sort preference.
    Sort,
}

/// Sparse provider-scoped routing input used by profile and request layers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OpenRouterRoutingPatch {
    source: Option<PolicySource>,
    allowed: Option<BTreeSet<UpstreamId>>,
    denied: Option<BTreeSet<UpstreamId>>,
    order: Option<Vec<UpstreamId>>,
    region: Option<(RoutingRegion, ConstraintStrength)>,
    data_retention: Option<(DataRetention, ConstraintStrength)>,
    fallback: Option<RoutingFallback>,
    sort: Option<RoutingSort>,
}

impl OpenRouterRoutingPatch {
    /// Creates an empty patch with an explicit source.
    #[must_use]
    pub const fn from_source(source: PolicySource) -> Self {
        Self {
            source: Some(source),
            allowed: None,
            denied: None,
            order: None,
            region: None,
            data_retention: None,
            fallback: None,
            sort: None,
        }
    }

    /// Sets the allowed upstream set. An empty set is rejected during resolution.
    #[must_use]
    pub fn with_allowed(mut self, values: impl IntoIterator<Item = UpstreamId>) -> Self {
        self.allowed = Some(values.into_iter().collect());
        self
    }

    /// Adds denied upstreams. Profile and request denials are unioned.
    #[must_use]
    pub fn with_denied(mut self, values: impl IntoIterator<Item = UpstreamId>) -> Self {
        self.denied = Some(values.into_iter().collect());
        self
    }

    /// Sets deterministic provider order. Duplicates are rejected during resolution.
    #[must_use]
    pub fn with_order(mut self, values: impl IntoIterator<Item = UpstreamId>) -> Self {
        self.order = Some(values.into_iter().collect());
        self
    }

    /// Sets an explicit region/residency requirement.
    #[must_use]
    pub fn with_region(mut self, region: RoutingRegion, strength: ConstraintStrength) -> Self {
        self.region = Some((region, strength));
        self
    }

    /// Sets an explicit data-retention requirement.
    #[must_use]
    pub const fn with_data_retention(
        mut self,
        retention: DataRetention,
        strength: ConstraintStrength,
    ) -> Self {
        self.data_retention = Some((retention, strength));
        self
    }

    /// Sets fallback behavior.
    #[must_use]
    pub fn with_fallback(mut self, fallback: RoutingFallback) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Sets provider sort preference.
    #[must_use]
    pub const fn with_sort(mut self, sort: RoutingSort) -> Self {
        self.sort = Some(sort);
        self
    }

    fn source(&self) -> PolicySource {
        self.source.unwrap_or(PolicySource::Request)
    }

    fn is_empty(&self) -> bool {
        self.allowed.is_none()
            && self.denied.is_none()
            && self.order.is_none()
            && self.region.is_none()
            && self.data_retention.is_none()
            && self.fallback.is_none()
            && self.sort.is_none()
    }
}

/// Request-level provider options kept outside the provider-neutral domain request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderRequestOptions {
    openrouter_routing: Option<OpenRouterRoutingPatch>,
}

impl ProviderRequestOptions {
    /// Creates empty provider options.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            openrouter_routing: None,
        }
    }

    /// Installs a typed OpenRouter-scoped routing patch.
    #[must_use]
    pub fn with_openrouter_routing(mut self, patch: OpenRouterRoutingPatch) -> Self {
        self.openrouter_routing = Some(patch);
        self
    }

    pub(crate) fn openrouter_routing(&self) -> Option<&OpenRouterRoutingPatch> {
        self.openrouter_routing.as_ref()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.openrouter_routing.is_none()
    }
}

/// Profile declaration that enables the private `OpenRouter` wire contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenRouterRoutingContract {
    defaults: OpenRouterRoutingPatch,
    region_wire_supported: bool,
}

impl OpenRouterRoutingContract {
    /// Creates a contract from provider-profile defaults.
    #[must_use]
    pub fn new(defaults: OpenRouterRoutingPatch) -> Self {
        Self {
            defaults,
            region_wire_supported: false,
        }
    }

    /// Enables a reviewed region wire mapping for a future provider profile.
    #[must_use]
    pub const fn with_region_wire_support(mut self, supported: bool) -> Self {
        self.region_wire_supported = supported;
        self
    }

    /// Resolves profile defaults and an optional request patch without I/O.
    ///
    /// # Errors
    ///
    /// Returns a validation error for conflicting inputs, weakened hard policy, or a field
    /// without a reviewed wire mapping.
    pub fn resolve(
        &self,
        request: Option<&OpenRouterRoutingPatch>,
    ) -> Result<ResolvedProviderRouting, LlmError> {
        let mut resolved = ResolvedProviderRouting::default();
        apply_patch(&mut resolved, &self.defaults, false)?;
        if let Some(request) = request {
            apply_patch(&mut resolved, request, true)?;
        }
        validate_resolved(&resolved, self.region_wire_supported)?;
        Ok(resolved)
    }
}

/// Complete routing policy consumed only by the private protocol adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedProviderRouting {
    pub(crate) allowed: BTreeSet<UpstreamId>,
    pub(crate) denied: BTreeSet<UpstreamId>,
    pub(crate) order: Vec<UpstreamId>,
    pub(crate) region: Option<(RoutingRegion, ConstraintStrength)>,
    pub(crate) data_retention: Option<(DataRetention, ConstraintStrength)>,
    pub(crate) fallback: Option<RoutingFallback>,
    pub(crate) sort: Option<RoutingSort>,
    pub(crate) provenance: BTreeMap<RoutingField, PolicySource>,
}

impl ResolvedProviderRouting {
    /// Returns the source of one resolved routing leaf.
    #[must_use]
    pub fn source(&self, field: RoutingField) -> Option<PolicySource> {
        self.provenance.get(&field).copied()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.allowed.is_empty()
            && self.denied.is_empty()
            && self.order.is_empty()
            && self.region.is_none()
            && self.data_retention.is_none()
            && self.fallback.is_none()
            && self.sort.is_none()
    }
}

fn apply_patch(
    resolved: &mut ResolvedProviderRouting,
    patch: &OpenRouterRoutingPatch,
    request_layer: bool,
) -> Result<(), LlmError> {
    if patch.is_empty() {
        return Ok(());
    }
    let source = patch.source();
    if let Some(allowed) = &patch.allowed {
        if allowed.is_empty() {
            return Err(conflict(
                "routing.allowed",
                "allowed upstream set cannot be empty",
            ));
        }
        resolved.allowed = if request_layer && !resolved.allowed.is_empty() {
            resolved.allowed.intersection(allowed).cloned().collect()
        } else {
            allowed.clone()
        };
        resolved
            .provenance
            .insert(RoutingField::AllowedUpstreams, source);
    }
    if let Some(denied) = &patch.denied {
        resolved.denied.extend(denied.iter().cloned());
        resolved
            .provenance
            .insert(RoutingField::DeniedUpstreams, source);
    }
    if let Some(order) = &patch.order {
        let unique = order.iter().collect::<BTreeSet<_>>();
        if unique.len() != order.len() {
            return Err(conflict(
                "routing.order",
                "provider order contains duplicates",
            ));
        }
        resolved.order.clone_from(order);
        resolved
            .provenance
            .insert(RoutingField::ProviderOrder, source);
    }
    if let Some(region) = &patch.region {
        if request_layer
            && let Some(existing) = &resolved.region
            && existing.1 == ConstraintStrength::Hard
            && region != existing
        {
            return Err(conflict(
                "routing.region",
                "request cannot replace a hard profile region constraint",
            ));
        }
        resolved.region = Some(region.clone());
        resolved.provenance.insert(RoutingField::Region, source);
    }
    if let Some(retention) = patch.data_retention {
        if request_layer
            && let Some(existing) = resolved.data_retention
            && existing.1 == ConstraintStrength::Hard
            && (retention.1 != ConstraintStrength::Hard
                || retention.0.strictness() < existing.0.strictness())
        {
            return Err(conflict(
                "routing.data_retention",
                "request cannot relax a hard profile retention constraint",
            ));
        }
        resolved.data_retention = Some(retention);
        resolved
            .provenance
            .insert(RoutingField::DataRetention, source);
    }
    if let Some(fallback) = &patch.fallback {
        if request_layer
            && resolved
                .fallback
                .as_ref()
                .is_some_and(|value| !value.enabled)
            && fallback.enabled
        {
            return Err(conflict(
                "routing.fallback",
                "request cannot enable fallback disabled by profile policy",
            ));
        }
        resolved.fallback = Some(fallback.clone());
        resolved.provenance.insert(RoutingField::Fallback, source);
    }
    if let Some(sort) = patch.sort {
        resolved.sort = Some(sort);
        resolved.provenance.insert(RoutingField::Sort, source);
    }
    Ok(())
}

fn validate_resolved(
    resolved: &ResolvedProviderRouting,
    region_wire_supported: bool,
) -> Result<(), LlmError> {
    if !resolved.allowed.is_disjoint(&resolved.denied) {
        return Err(conflict(
            "routing.upstreams",
            "allowed and denied upstream sets overlap",
        ));
    }
    if !resolved.allowed.is_empty()
        && resolved
            .order
            .iter()
            .any(|provider| !resolved.allowed.contains(provider))
    {
        return Err(conflict(
            "routing.order",
            "provider order contains an upstream outside the allowlist",
        ));
    }
    if resolved
        .order
        .iter()
        .any(|item| resolved.denied.contains(item))
    {
        return Err(conflict(
            "routing.order",
            "provider order contains a denied upstream",
        ));
    }
    if resolved.region.is_some() && !region_wire_supported {
        return Err(validation(
            "routing.region",
            ValidationReason::CapabilityUnsupported,
            "profile has no reviewed region wire mapping",
        )
        .into());
    }
    Ok(())
}

fn validation(
    field: &'static str,
    reason: ValidationReason,
    summary: &'static str,
) -> ValidationError {
    ValidationError::new(field, reason, summary)
}

fn conflict(field: &'static str, summary: &'static str) -> LlmError {
    validation(field, ValidationReason::Conflict, summary).into()
}
