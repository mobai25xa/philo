//! Restricted path variables and deterministic registered-query operations.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use url::Url;

use crate::error::LlmError;
use crate::provider::catalog::{DeploymentId, ProductId, ProviderModelId};

/// Closed set of variables accepted by [`EndpointTemplate`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EndpointPathVariable {
    /// Provider product identifier.
    Product,
    /// Provider-owned model identifier.
    ProviderModel,
    /// Deployment identifier; resolution fails when absent.
    Deployment,
}

/// Typed values available while resolving one model target.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_field_names)]
pub struct EndpointValues<'a> {
    product_id: &'a ProductId,
    provider_model_id: &'a ProviderModelId,
    deployment_id: Option<&'a DeploymentId>,
}

impl<'a> EndpointValues<'a> {
    /// Creates values from a compiled catalog mapping.
    pub const fn new(
        product_id: &'a ProductId,
        provider_model_id: &'a ProviderModelId,
        deployment_id: Option<&'a DeploymentId>,
    ) -> Self {
        Self {
            product_id,
            provider_model_id,
            deployment_id,
        }
    }

    fn value(self, variable: EndpointPathVariable) -> Result<&'a str, LlmError> {
        match variable {
            EndpointPathVariable::Product => Ok(self.product_id.as_str()),
            EndpointPathVariable::ProviderModel => Ok(self.provider_model_id.as_str()),
            EndpointPathVariable::Deployment => self
                .deployment_id
                .map(DeploymentId::as_str)
                .ok_or_else(|| configuration("endpoint template requires a deployment id")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PathSegment {
    Literal(String),
    Variable(EndpointPathVariable),
}

/// A path-only template with a closed variable vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointTemplate {
    segments: Vec<PathSegment>,
}

impl EndpointTemplate {
    /// Parses a relative path template.
    ///
    /// Accepted variables are `{product}`, `{provider_model}`, and `{deployment}`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, LlmError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(configuration("endpoint template must not be empty"));
        }
        if value.contains(['?', '#']) {
            return Err(configuration(
                "endpoint template must not contain query or fragment",
            ));
        }
        let value = value.trim_matches('/');
        if value.is_empty() {
            return Err(configuration("endpoint template must contain a path"));
        }
        let mut segments = Vec::new();
        for segment in value.split('/') {
            if segment.is_empty() || matches!(segment, "." | "..") {
                return Err(configuration(
                    "endpoint template has an unsafe path segment",
                ));
            }
            let parsed = match segment {
                "{product}" => PathSegment::Variable(EndpointPathVariable::Product),
                "{provider_model}" => PathSegment::Variable(EndpointPathVariable::ProviderModel),
                "{deployment}" => PathSegment::Variable(EndpointPathVariable::Deployment),
                literal if literal.contains(['{', '}']) || literal.contains('%') => {
                    return Err(configuration(
                        "endpoint template contains an unknown or encoded segment",
                    ));
                }
                literal => PathSegment::Literal(literal.to_owned()),
            };
            segments.push(parsed);
        }
        Ok(Self { segments })
    }

    pub(crate) fn render(
        &self,
        base_path: &str,
        values: Option<EndpointValues<'_>>,
    ) -> Result<(String, Vec<EndpointPathVariable>), LlmError> {
        let mut path = base_path.trim_end_matches('/').to_owned();
        if path.is_empty() {
            path.push('/');
        } else if !path.starts_with('/') {
            path.insert(0, '/');
        }
        let mut used = Vec::new();
        for segment in &self.segments {
            if !path.ends_with('/') {
                path.push('/');
            }
            match segment {
                PathSegment::Literal(value) => path.push_str(value),
                PathSegment::Variable(variable) => {
                    let values = values.ok_or_else(|| {
                        configuration("endpoint template requires catalog mapping values")
                    })?;
                    path.push_str(&encode_path_segment(values.value(*variable)?));
                    used.push(*variable);
                }
            }
        }
        Ok((path, used))
    }

    pub(crate) fn requires_values(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| matches!(segment, PathSegment::Variable(_)))
    }
}

/// Provenance label retained for endpoint query diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointQuerySource {
    /// Provider product profile.
    ProductProfile,
    /// Deployment mapping.
    DeploymentMapping,
    /// Explicit local test configuration.
    TestConfiguration,
}

/// Deterministic behavior when a registered key already exists on the base URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryMergeRule {
    /// Reject an existing value.
    RejectExisting,
    /// Replace an existing value.
    Override,
}

/// Value-free query action recorded in endpoint diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointQueryAction {
    /// A value was set.
    Set,
    /// A value was removed.
    Remove,
}

/// Value-free query resolution record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointQueryDiagnostic {
    name: String,
    action: EndpointQueryAction,
    source: EndpointQuerySource,
}

impl EndpointQueryDiagnostic {
    /// Returns the non-sensitive query name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the applied action.
    pub const fn action(&self) -> EndpointQueryAction {
        self.action
    }

    /// Returns the configuration source.
    pub const fn source(&self) -> EndpointQuerySource {
        self.source
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum QueryOperation {
    Set {
        value: String,
        merge: QueryMergeRule,
        source: EndpointQuerySource,
    },
    Remove {
        source: EndpointQuerySource,
    },
}

/// Explicit query-key registry and merge plan.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct EndpointQuery {
    operations: BTreeMap<String, QueryOperation>,
}

impl fmt::Debug for EndpointQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operations = self
            .operations
            .iter()
            .map(|(name, operation)| {
                let (action, source) = match operation {
                    QueryOperation::Set { source, .. } => (EndpointQueryAction::Set, *source),
                    QueryOperation::Remove { source } => (EndpointQueryAction::Remove, *source),
                };
                EndpointQueryDiagnostic {
                    name: name.clone(),
                    action,
                    source,
                }
            })
            .collect::<Vec<_>>();
        formatter
            .debug_struct("EndpointQuery")
            .field("operations", &operations)
            .finish()
    }
}

impl EndpointQuery {
    /// Creates an empty query plan.
    pub const fn new() -> Self {
        Self {
            operations: BTreeMap::new(),
        }
    }

    /// Registers a key and sets its final value.
    pub fn with_set(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        merge: QueryMergeRule,
        source: EndpointQuerySource,
    ) -> Result<Self, LlmError> {
        let name = name.into();
        let name = validate_query_name(&name)?;
        let value = validate_query_value(value.into())?;
        if self
            .operations
            .insert(
                name,
                QueryOperation::Set {
                    value,
                    merge,
                    source,
                },
            )
            .is_some()
        {
            return Err(configuration("duplicate endpoint query operation"));
        }
        Ok(self)
    }

    /// Registers a key and removes it from a base URL.
    pub fn with_remove(
        mut self,
        name: impl Into<String>,
        source: EndpointQuerySource,
    ) -> Result<Self, LlmError> {
        let name = name.into();
        let name = validate_query_name(&name)?;
        if self
            .operations
            .insert(name, QueryOperation::Remove { source })
            .is_some()
        {
            return Err(configuration("duplicate endpoint query operation"));
        }
        Ok(self)
    }

    /// Registers the conventional `api-version` key with deterministic override behavior.
    pub fn with_api_version(
        self,
        value: impl Into<String>,
        source: EndpointQuerySource,
    ) -> Result<Self, LlmError> {
        self.with_set("api-version", value, QueryMergeRule::Override, source)
    }

    pub(crate) fn apply(&self, url: &mut Url) -> Result<Vec<EndpointQueryDiagnostic>, LlmError> {
        let mut existing = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (name, value) in url.query_pairs() {
            let name = name.into_owned();
            validate_query_name(&name)?;
            if !seen.insert(name.clone()) {
                return Err(configuration("duplicate base endpoint query key"));
            }
            if !self.operations.contains_key(&name) {
                return Err(configuration("base endpoint query key is not registered"));
            }
            existing.insert(name, value.into_owned());
        }

        let mut diagnostics = Vec::with_capacity(self.operations.len());
        for (name, operation) in &self.operations {
            match operation {
                QueryOperation::Set {
                    value,
                    merge,
                    source,
                } => {
                    if existing.contains_key(name) && *merge == QueryMergeRule::RejectExisting {
                        return Err(configuration("endpoint query override is forbidden"));
                    }
                    existing.insert(name.clone(), value.clone());
                    diagnostics.push(EndpointQueryDiagnostic {
                        name: name.clone(),
                        action: EndpointQueryAction::Set,
                        source: *source,
                    });
                }
                QueryOperation::Remove { source } => {
                    existing.remove(name);
                    diagnostics.push(EndpointQueryDiagnostic {
                        name: name.clone(),
                        action: EndpointQueryAction::Remove,
                        source: *source,
                    });
                }
            }
        }
        url.set_query(None);
        if !existing.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in existing {
                pairs.append_pair(&name, &value);
            }
        }
        Ok(diagnostics)
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn validate_query_name(name: &str) -> Result<String, LlmError> {
    let canonical = name.to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 64
        || !name.is_ascii()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(configuration("invalid endpoint query name"));
    }
    if [
        "key",
        "token",
        "secret",
        "credential",
        "signature",
        "authorization",
    ]
    .iter()
    .any(|sensitive| canonical.contains(sensitive))
    {
        return Err(configuration("sensitive endpoint query name is forbidden"));
    }
    Ok(canonical)
}

fn validate_query_value(value: String) -> Result<String, LlmError> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == 0x7f)
    {
        return Err(configuration("invalid endpoint query value"));
    }
    Ok(value)
}

fn configuration(message: &'static str) -> LlmError {
    LlmError::Configuration(message.to_owned())
}
