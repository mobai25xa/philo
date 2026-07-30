//! Secret handling, Bearer authentication, and truthful client identity.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use http::{HeaderMap, HeaderName, HeaderValue, header};
use secrecy::{ExposeSecret, SecretString};

use super::catalog::ProductId;
use super::endpoint::{CredentialBinding, ResolvedEndpoint};
pub use super::headers::ClientIdentity;
use super::headers::HeaderOperation;
use crate::domain::ProviderId;
use crate::error::{LlmError, ValidationError, ValidationReason};
use crate::transport::RequestLifecycle;

mod cache;
mod dynamic;
mod providers;

pub use cache::DynamicCredentialCache;
pub use dynamic::{
    CredentialFuture, CredentialIdentity, DynamicAuth, DynamicCredential, DynamicCredentialContext,
    DynamicCredentialScheme, DynamicCredentialSource, TenantId,
};
pub use providers::{ApiKeyHeaderAuth, MultiHeaderAuth, NoAuth};

/// Value-free authentication shape exposed to diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthSchemeKind {
    /// `Authorization: Bearer <secret>`.
    Bearer,
    /// One explicitly registered API-key header.
    ApiKeyHeader,
    /// An atomic group of authentication headers.
    MultiHeader,
    /// A dynamic source whose allowed schemes are validated at runtime.
    Dynamic,
    /// Explicit unauthenticated operation.
    None,
    /// A downstream implementation that did not provide a more specific shape.
    Custom,
}

/// Value-free credential source class exposed to diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSourceKind {
    /// Credential material was provided when the profile was built.
    Static,
    /// Credential material is acquired through a bounded callback and cache.
    Dynamic,
    /// The profile intentionally has no credential.
    None,
    /// A downstream implementation that did not declare its source class.
    Custom,
}

/// API key wrapper whose Debug and Display representations are always redacted.
#[derive(Clone)]
pub struct ApiKey(SecretString);

impl ApiKey {
    /// Creates a non-empty Bearer-compatible secret without trimming it.
    pub fn new(value: impl Into<String>) -> Result<Self, LlmError> {
        let value = value.into();
        if value.is_empty() {
            return Err(validation(
                "api_key",
                ValidationReason::Empty,
                "API key must not be empty",
            ));
        }
        if value.trim() != value {
            return Err(validation(
                "api_key",
                ValidationReason::BoundaryWhitespace,
                "API key must not have boundary whitespace",
            ));
        }
        HeaderValue::from_str(&format!("Bearer {value}")).map_err(|_| {
            validation(
                "api_key",
                ValidationReason::InvalidHeader,
                "API key is not valid in a Bearer header",
            )
        })?;
        Ok(Self(SecretString::from(value)))
    }
    pub(crate) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Bearer credential bound to an immutable destination.
#[derive(Clone)]
pub struct BearerCredential {
    key: ApiKey,
    binding: CredentialBinding,
}

impl BearerCredential {
    /// Binds a secret to its allowed destination.
    pub fn new(key: ApiKey, binding: impl Into<CredentialBinding>) -> Self {
        Self {
            key,
            binding: binding.into(),
        }
    }
    /// Returns the credential destination binding.
    pub fn binding(&self) -> &CredentialBinding {
        &self.binding
    }

    /// Compatibility alias for [`Self::binding`].
    pub fn audience(&self) -> &CredentialBinding {
        self.binding()
    }
}

impl fmt::Debug for BearerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerCredential")
            .field("key", &"[REDACTED]")
            .field("binding", &self.binding)
            .finish()
    }
}

/// Safe authentication context containing no prompt or wire request.
#[derive(Clone, Copy, Debug)]
pub struct AuthContext<'a> {
    endpoint: &'a ResolvedEndpoint,
    provider_id: Option<&'a ProviderId>,
    product_id: Option<&'a ProductId>,
    lifecycle: Option<&'a RequestLifecycle>,
}

impl<'a> AuthContext<'a> {
    /// Creates an authentication context for a resolved endpoint.
    pub fn new(endpoint: &'a ResolvedEndpoint) -> Self {
        Self {
            endpoint,
            provider_id: None,
            product_id: None,
            lifecycle: None,
        }
    }
    /// Returns the final endpoint.
    pub fn endpoint(&self) -> &'a ResolvedEndpoint {
        self.endpoint
    }

    /// Returns the provider identifier when resolving a real attempt.
    pub fn provider_id(&self) -> Option<&'a ProviderId> {
        self.provider_id
    }

    /// Returns the product identifier when resolving a real attempt.
    pub fn product_id(&self) -> Option<&'a ProductId> {
        self.product_id
    }

    /// Returns the request lifecycle when resolving a real attempt.
    pub fn lifecycle(&self) -> Option<&'a RequestLifecycle> {
        self.lifecycle
    }

    pub(crate) const fn for_attempt(
        endpoint: &'a ResolvedEndpoint,
        provider_id: &'a ProviderId,
        product_id: &'a ProductId,
        lifecycle: &'a RequestLifecycle,
    ) -> Self {
        Self {
            endpoint,
            provider_id: Some(provider_id),
            product_id: Some(product_id),
            lifecycle: Some(lifecycle),
        }
    }
}

/// Future returned by an authentication provider.
pub type AuthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<HeaderOperation>, LlmError>> + Send + 'a>>;

/// Authentication source that contributes only protected auth headers.
pub trait AuthProvider: fmt::Debug + Send + Sync {
    /// Resolves a complete, atomic group of authentication headers.
    fn resolve<'a>(&'a self, context: AuthContext<'a>) -> AuthFuture<'a>;

    /// Resolves an immediately available credential for compatibility-only sync APIs.
    fn resolve_immediate(
        &self,
        _context: AuthContext<'_>,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        Err(crate::error::CredentialError::new(crate::error::CredentialFailure::Unavailable).into())
    }

    /// Returns every header name owned by this authentication provider.
    fn protected_headers(&self) -> Vec<HeaderName>;

    /// Returns the immutable destination binding when this provider carries credentials.
    fn credential_binding(&self) -> Option<&CredentialBinding> {
        None
    }

    /// Validates credential binding before the runtime becomes usable.
    fn validate_endpoint(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError>;

    /// Validates the final authentication fields for this concrete scheme.
    fn validate_final(&self, _headers: &HeaderMap) -> Result<(), LlmError> {
        Ok(())
    }

    /// Returns the authentication shape without resolving or exposing a credential.
    fn scheme_kind(&self) -> AuthSchemeKind {
        AuthSchemeKind::Custom
    }

    /// Returns the credential source class without resolving or exposing a credential.
    fn credential_source_kind(&self) -> CredentialSourceKind {
        CredentialSourceKind::Custom
    }
}

/// Bearer authentication provider.
#[derive(Clone)]
pub struct BearerAuth {
    credential: BearerCredential,
}

impl BearerAuth {
    /// Creates a Bearer authentication provider.
    pub fn new(credential: BearerCredential) -> Self {
        Self { credential }
    }
    /// Returns the bound credential destination.
    pub fn binding(&self) -> &CredentialBinding {
        self.credential.binding()
    }

    /// Compatibility alias for [`Self::binding`].
    pub fn audience(&self) -> &CredentialBinding {
        self.binding()
    }

    /// Produces the Bearer header after validating the credential binding.
    pub fn operation(&self, context: AuthContext<'_>) -> Result<HeaderOperation, LlmError> {
        self.credential.binding.validate(context.endpoint())?;
        let value = HeaderValue::from_str(&format!("Bearer {}", self.credential.key.expose()))
            .map_err(|_| {
                validation(
                    "authorization",
                    ValidationReason::InvalidHeader,
                    "Bearer credential cannot form a header",
                )
            })?;
        Ok(HeaderOperation::set_sensitive(header::AUTHORIZATION, value))
    }
}

impl fmt::Debug for BearerAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerAuth")
            .field("credential", &self.credential)
            .finish()
    }
}

impl AuthProvider for BearerAuth {
    fn resolve<'a>(&'a self, context: AuthContext<'a>) -> AuthFuture<'a> {
        Box::pin(async move { Ok(vec![self.operation(context)?]) })
    }

    fn resolve_immediate(
        &self,
        context: AuthContext<'_>,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        Ok(vec![self.operation(context)?])
    }

    fn protected_headers(&self) -> Vec<HeaderName> {
        vec![header::AUTHORIZATION]
    }

    fn credential_binding(&self) -> Option<&CredentialBinding> {
        Some(&self.credential.binding)
    }

    fn validate_endpoint(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        self.credential.binding.validate(endpoint)
    }

    fn validate_final(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        let valid = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Bearer ") && value.len() > 7);
        if valid {
            Ok(())
        } else {
            Err(validation(
                "request_headers.authorization",
                ValidationReason::ProtectedHeader,
                "Bearer auth provider must set authorization",
            ))
        }
    }

    fn scheme_kind(&self) -> AuthSchemeKind {
        AuthSchemeKind::Bearer
    }

    fn credential_source_kind(&self) -> CredentialSourceKind {
        CredentialSourceKind::Static
    }
}

fn validation(
    field: impl Into<String>,
    reason: ValidationReason,
    summary: &'static str,
) -> LlmError {
    ValidationError::new(field, reason, summary).into()
}
