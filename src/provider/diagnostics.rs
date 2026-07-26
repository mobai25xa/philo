//! Value-free diagnostics for a compiled provider call policy.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use http::HeaderName;

use crate::domain::{ModelId, PolicySource, ProtocolId, ProviderId};
use crate::error::LlmError;

use super::auth::{AuthSchemeKind, CredentialSourceKind};
use super::call_policy::CallPolicySnapshot;
use super::capability::ProviderCapabilities;
use super::catalog::{DeploymentId, ModelEntry, ProductId, ProviderModelId, SupportStatus};
use super::compat::{CompatField, CompatProfile};
use super::endpoint::{Origin, ResolvedEndpoint};
use super::headers::HeaderSource;

/// Effective support state after applying evidence expiry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveSupportStatus {
    /// Exact support is current and verified.
    Supported,
    /// The exact integration is intentionally unstable or not yet fully verified.
    Experimental,
    /// The exact capability or target is explicitly unavailable.
    Unsupported,
    /// No support conclusion is available.
    Unknown,
    /// A previous declaration exists but its evidence has expired.
    Stale,
}

/// Evidence verification class used by diagnostics and the support matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceVerification {
    /// The runtime can only identify its catalog declaration.
    CatalogDeclaration,
    /// Synthetic fixtures and the shared offline contract passed.
    OfflineContractVerified,
    /// A protected exact-model provider run passed.
    RealProviderVerified,
}

/// One final compatibility decision and its source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatDiagnostic {
    field: CompatField,
    value: String,
    source: PolicySource,
}

impl CompatDiagnostic {
    /// Returns the stable compatibility field.
    pub const fn field(&self) -> CompatField {
        self.field
    }

    /// Returns a closed-enum decision label, never request content.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the winning policy layer.
    pub const fn source(&self) -> PolicySource {
        self.source
    }
}

/// One value-free Header owner declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderDiagnostic {
    name: HeaderName,
    source: HeaderSource,
    protected: bool,
    sensitive: bool,
}

impl HeaderDiagnostic {
    /// Returns the canonical Header name.
    pub fn name(&self) -> &HeaderName {
        &self.name
    }

    /// Returns the owner layer.
    pub const fn source(&self) -> HeaderSource {
        self.source
    }

    /// Returns whether ordinary layers are forbidden from modifying the Header.
    pub const fn is_protected(&self) -> bool {
        self.protected
    }

    /// Returns whether the omitted value is credential material.
    pub const fn is_sensitive(&self) -> bool {
        self.sensitive
    }
}

/// Authentication diagnostics that never resolve a credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthDiagnostics {
    scheme: AuthSchemeKind,
    credential_source: CredentialSourceKind,
    protected_headers: Vec<HeaderName>,
}

impl AuthDiagnostics {
    /// Returns the declared authentication shape.
    pub const fn scheme(&self) -> AuthSchemeKind {
        self.scheme
    }

    /// Returns the credential source class.
    pub const fn credential_source(&self) -> CredentialSourceKind {
        self.credential_source
    }

    /// Returns protected Header names without their values.
    pub fn protected_headers(&self) -> &[HeaderName] {
        &self.protected_headers
    }
}

/// Redacted endpoint selection and resolution facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointDiagnostics {
    origin: Origin,
    path_shape: String,
    path_variables: Vec<String>,
    query_names: Vec<String>,
    source: PolicySource,
}

impl EndpointDiagnostics {
    /// Returns the normalized scheme/host/effective port.
    pub const fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Returns a fixed path or a variable-name-only template marker.
    pub fn path_shape(&self) -> &str {
        &self.path_shape
    }

    /// Returns typed path-variable labels without resolved values.
    pub fn path_variables(&self) -> &[String] {
        &self.path_variables
    }

    /// Returns registered query names without values.
    pub fn query_names(&self) -> &[String] {
        &self.query_names
    }

    /// Returns the endpoint policy source.
    pub const fn source(&self) -> PolicySource {
        self.source
    }
}

/// Exact catalog evidence and freshness facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupportDiagnostics {
    status: EffectiveSupportStatus,
    verification: EvidenceVerification,
    evidence_id: Option<String>,
    reviewed_at: Option<String>,
    expires_at: Option<String>,
}

impl SupportDiagnostics {
    /// Returns the effective five-state support conclusion.
    pub const fn status(&self) -> EffectiveSupportStatus {
        self.status
    }

    /// Returns what kind of evidence this snapshot itself can prove.
    pub const fn verification(&self) -> EvidenceVerification {
        self.verification
    }

    /// Returns the catalog evidence identifier.
    pub fn evidence_id(&self) -> Option<&str> {
        self.evidence_id.as_deref()
    }

    /// Returns the evidence review date.
    pub fn reviewed_at(&self) -> Option<&str> {
        self.reviewed_at.as_deref()
    }

    /// Returns the evidence expiry date.
    pub fn expires_at(&self) -> Option<&str> {
        self.expires_at.as_deref()
    }
}

/// Value-free explanation of one compiled provider call policy.
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderDiagnostics {
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    domain_model: ModelId,
    provider_model: ProviderModelId,
    deployment_id: Option<DeploymentId>,
    wire_model: ModelId,
    endpoint: EndpointDiagnostics,
    auth: AuthDiagnostics,
    headers: Vec<HeaderDiagnostic>,
    capabilities: ProviderCapabilities,
    compat: Vec<CompatDiagnostic>,
    typed_extensions: Vec<&'static str>,
    support: SupportDiagnostics,
}

impl ProviderDiagnostics {
    /// Returns the selected provider.
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the selected product.
    pub const fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the selected protocol.
    pub const fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }

    /// Returns the caller-facing exact model.
    pub const fn domain_model(&self) -> &ModelId {
        &self.domain_model
    }

    /// Returns the provider-owned exact model.
    pub const fn provider_model(&self) -> &ProviderModelId {
        &self.provider_model
    }

    /// Returns the optional deployment mapping.
    pub const fn deployment_id(&self) -> Option<&DeploymentId> {
        self.deployment_id.as_ref()
    }

    /// Returns the exact model value used by the private wire encoder.
    pub const fn wire_model(&self) -> &ModelId {
        &self.wire_model
    }

    /// Returns redacted endpoint facts.
    pub const fn endpoint(&self) -> &EndpointDiagnostics {
        &self.endpoint
    }

    /// Returns value-free authentication facts.
    pub const fn auth(&self) -> &AuthDiagnostics {
        &self.auth
    }

    /// Returns Header names, owners, and protection flags without values.
    pub fn headers(&self) -> &[HeaderDiagnostic] {
        &self.headers
    }

    /// Returns the exact model capability snapshot.
    pub const fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    /// Returns every final typed compatibility decision and provenance.
    pub fn compat(&self) -> &[CompatDiagnostic] {
        &self.compat
    }

    /// Returns enabled provider-scoped typed extension labels.
    pub fn typed_extensions(&self) -> &[&'static str] {
        &self.typed_extensions
    }

    /// Returns exact catalog support and evidence freshness.
    pub const fn support(&self) -> &SupportDiagnostics {
        &self.support
    }

    pub(crate) fn compile(input: DiagnosticsInput<'_>) -> Result<Self, LlmError> {
        let endpoint = endpoint_diagnostics(input.endpoint);
        let headers = header_diagnostics(&input);
        let support = support_diagnostics(input.entry, input.as_of)?;
        let compat = input
            .plan
            .compat
            .profile
            .as_ref()
            .map_or_else(Vec::new, |profile| {
                CompatField::all()
                    .into_iter()
                    .map(|field| CompatDiagnostic {
                        field,
                        value: compat_value(profile, field),
                        source: profile.source(field),
                    })
                    .collect()
            });
        let mut typed_extensions: Vec<&'static str> = input
            .plan
            .provider_routing
            .is_some()
            .then_some("openrouter-routing")
            .into_iter()
            .collect();
        typed_extensions.extend(input.protocol_option_labels);

        Ok(Self {
            provider_id: input.plan.target.provider_id.clone(),
            product_id: input.plan.target.product_id.clone(),
            protocol_id: input.plan.target.protocol_id.clone(),
            domain_model: input.plan.target.domain_model.clone(),
            provider_model: input.plan.target.provider_model.clone(),
            deployment_id: input.plan.target.deployment_id.clone(),
            wire_model: input.plan.target.wire_model.clone(),
            endpoint,
            auth: AuthDiagnostics {
                scheme: input.auth_scheme,
                credential_source: input.credential_source,
                protected_headers: input.auth_headers,
            },
            headers,
            capabilities: input.plan.capabilities.clone(),
            compat,
            typed_extensions,
            support,
        })
    }
}

impl fmt::Debug for ProviderDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderDiagnostics")
            .field("provider_id", &self.provider_id)
            .field("product_id", &self.product_id)
            .field("protocol_id", &self.protocol_id)
            .field("domain_model", &self.domain_model)
            .field("provider_model", &self.provider_model)
            .field("deployment_id", &self.deployment_id)
            .field("wire_model", &self.wire_model)
            .field("endpoint", &self.endpoint)
            .field("auth", &self.auth)
            .field("headers", &self.headers)
            .field("capabilities", &self.capabilities)
            .field("compat", &self.compat)
            .field("typed_extensions", &self.typed_extensions)
            .field("support", &self.support)
            .finish()
    }
}

impl fmt::Display for ProviderDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "provider={}/{} model={} protocol={} support={:?}",
            self.provider_id,
            self.product_id,
            self.domain_model,
            self.protocol_id,
            self.support.status
        )
    }
}

pub(crate) struct DiagnosticsInput<'a> {
    pub(crate) plan: &'a CallPolicySnapshot,
    pub(crate) endpoint: &'a ResolvedEndpoint,
    pub(crate) entry: Option<&'a ModelEntry>,
    pub(crate) as_of: &'a str,
    pub(crate) auth_scheme: AuthSchemeKind,
    pub(crate) credential_source: CredentialSourceKind,
    pub(crate) auth_headers: Vec<HeaderName>,
    pub(crate) provider_headers: Vec<HeaderName>,
    pub(crate) model_headers: Vec<HeaderName>,
    pub(crate) dynamic_headers: Vec<HeaderName>,
    pub(crate) request_headers: Vec<HeaderName>,
    pub(crate) protocol_option_labels: Vec<&'static str>,
}

fn endpoint_diagnostics(endpoint: &ResolvedEndpoint) -> EndpointDiagnostics {
    let path_variables = endpoint
        .diagnostics()
        .path_variables()
        .iter()
        .map(|variable| format!("{variable:?}"))
        .collect::<Vec<_>>();
    let path_shape = if path_variables.is_empty() {
        endpoint.url().path().to_owned()
    } else {
        format!("templated:[{}]", path_variables.join(","))
    };
    let query_names = endpoint
        .diagnostics()
        .query()
        .iter()
        .map(|entry| entry.name().to_owned())
        .collect();
    EndpointDiagnostics {
        origin: endpoint.origin().clone(),
        path_shape,
        path_variables,
        query_names,
        source: PolicySource::ProviderProfile,
    }
}

fn header_diagnostics(input: &DiagnosticsInput<'_>) -> Vec<HeaderDiagnostic> {
    let mut headers = vec![
        header("content-type", HeaderSource::Protocol, true, false),
        header("accept", HeaderSource::Protocol, true, false),
        header("user-agent", HeaderSource::ClientIdentity, true, false),
    ];
    extend_headers(
        &mut headers,
        &input.provider_headers,
        HeaderSource::Provider,
        true,
        false,
    );
    extend_headers(
        &mut headers,
        &input.model_headers,
        HeaderSource::Model,
        false,
        false,
    );
    extend_headers(
        &mut headers,
        &input.dynamic_headers,
        HeaderSource::DynamicPolicy,
        false,
        false,
    );
    extend_headers(
        &mut headers,
        &input.request_headers,
        HeaderSource::Request,
        false,
        false,
    );
    extend_headers(
        &mut headers,
        &input.auth_headers,
        HeaderSource::Auth,
        true,
        true,
    );
    headers.sort_by(|left, right| {
        left.name
            .as_str()
            .cmp(right.name.as_str())
            .then(left.source.cmp(&right.source))
    });
    headers.dedup();
    headers
}

fn extend_headers(
    diagnostics: &mut Vec<HeaderDiagnostic>,
    names: &[HeaderName],
    source: HeaderSource,
    protected: bool,
    sensitive: bool,
) {
    diagnostics.extend(names.iter().cloned().map(|name| HeaderDiagnostic {
        name,
        source,
        protected,
        sensitive,
    }));
}

fn support_diagnostics(
    entry: Option<&ModelEntry>,
    as_of: &str,
) -> Result<SupportDiagnostics, LlmError> {
    let Some(entry) = entry else {
        return Ok(SupportDiagnostics {
            status: EffectiveSupportStatus::Unknown,
            verification: EvidenceVerification::CatalogDeclaration,
            evidence_id: None,
            reviewed_at: None,
            expires_at: None,
        });
    };
    let stale = entry.source.is_stale_on(as_of)?;
    let status = if stale {
        EffectiveSupportStatus::Stale
    } else {
        match entry.support_status {
            SupportStatus::Supported => EffectiveSupportStatus::Supported,
            SupportStatus::Experimental => EffectiveSupportStatus::Experimental,
            SupportStatus::Unsupported => EffectiveSupportStatus::Unsupported,
            SupportStatus::Unknown => EffectiveSupportStatus::Unknown,
        }
    };
    Ok(SupportDiagnostics {
        status,
        verification: EvidenceVerification::CatalogDeclaration,
        evidence_id: Some(entry.source.id().as_str().to_owned()),
        reviewed_at: Some(entry.source.reviewed_at().to_owned()),
        expires_at: entry.source.expires_at().map(str::to_owned),
    })
}

fn header(
    name: &'static str,
    source: HeaderSource,
    protected: bool,
    sensitive: bool,
) -> HeaderDiagnostic {
    HeaderDiagnostic {
        name: HeaderName::from_static(name),
        source,
        protected,
        sensitive,
    }
}

fn compat_value(profile: &CompatProfile, field: CompatField) -> String {
    match field {
        CompatField::RequestModelBody => format!("{:?}", profile.request().model_body),
        CompatField::RequestMaxOutputTokens => {
            format!("{:?}", profile.request().max_output_tokens)
        }
        CompatField::RequestToolChoice => format!("{:?}", profile.request().tool_choice),
        CompatField::RequestThinking => format!("{:?}", profile.request().thinking),
        CompatField::RequestImage => format!("{:?}", profile.request().image),
        CompatField::RequestStreamUsage => format!("{:?}", profile.request().stream_usage),
        CompatField::RequestStructuredOutput => {
            format!("{:?}", profile.request().structured_output)
        }
        CompatField::ResponseFinishReason => format!("{:?}", profile.response().finish_reason),
        CompatField::ResponseToolArguments => {
            format!("{:?}", profile.response().tool_arguments)
        }
        CompatField::ResponseUsage => format!("{:?}", profile.response().usage),
        CompatField::ResponseInlineError => format!("{:?}", profile.response().inline_error),
        CompatField::HistoryMissingToolResult => {
            format!("{:?}", profile.history().missing_tool_result)
        }
        CompatField::HistoryUnsupportedContent => {
            format!("{:?}", profile.history().unsupported_content)
        }
        CompatField::HistoryThinkingReplay => {
            format!("{:?}", profile.history().thinking_replay)
        }
        CompatField::HistoryToolResultName => {
            format!("{:?}", profile.history().tool_result_name)
        }
        CompatField::HistoryToolCallId => format!("{:?}", profile.history().tool_call_id),
    }
}
