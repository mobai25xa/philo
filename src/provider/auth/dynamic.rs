//! Cancellable dynamic credentials with monotonic expiry.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, header};
use tokio::time::Instant;

use super::cache::AuthCacheKey;
use super::providers::validate_auth_name;
use super::{ApiKey, AuthContext, AuthFuture, AuthProvider, DynamicCredentialCache};
use crate::domain::ProviderId;
use crate::error::{
    CredentialError, CredentialFailure, LlmError, ValidationError, ValidationReason,
};
use crate::provider::catalog::ProductId;
use crate::provider::endpoint::{CredentialAudience, ResolvedEndpoint};
use crate::provider::headers::HeaderOperation;
use crate::transport::{CancellationToken, await_with_lifecycle};

macro_rules! private_id {
    ($name:ident, $field:literal) => {
        #[doc = concat!("Opaque ", $field, " used only for credential cache isolation.")]
        #[derive(Clone, Eq, Hash, PartialEq)]
        pub struct $name(String);

        impl $name {
            /// Creates a non-empty opaque identifier of at most 256 bytes.
            pub fn new(value: impl Into<String>) -> Result<Self, LlmError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(ValidationError::new(
                        $field,
                        ValidationReason::Empty,
                        "identifier must not be empty",
                    )
                    .into());
                }
                if value.trim() != value {
                    return Err(ValidationError::new(
                        $field,
                        ValidationReason::BoundaryWhitespace,
                        "identifier must not have boundary whitespace",
                    )
                    .into());
                }
                if value.len() > 256 {
                    return Err(ValidationError::new(
                        $field,
                        ValidationReason::OutOfRange,
                        "identifier exceeds 256 bytes",
                    )
                    .into());
                }
                Ok(Self(value))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("[REDACTED]")
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("[REDACTED]")
            }
        }
    };
}

private_id!(TenantId, "tenant_id");
private_id!(CredentialIdentity, "credential_identity");

/// Header scheme supplied by a dynamic credential source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DynamicCredentialScheme {
    /// `Authorization: Bearer <secret>`.
    Bearer,
    /// Raw secret in an explicitly registered authentication header.
    ApiKeyHeader(HeaderName),
}

/// Short-lived secret and its monotonic expiration instant.
#[derive(Clone)]
pub struct DynamicCredential {
    key: ApiKey,
    scheme: DynamicCredentialScheme,
    expires_at: Instant,
}

impl DynamicCredential {
    /// Creates a short-lived Bearer credential.
    pub fn bearer(key: ApiKey, expires_at: Instant) -> Result<Self, CredentialError> {
        Self::new(key, DynamicCredentialScheme::Bearer, expires_at)
    }

    /// Creates a short-lived raw API-key header credential.
    pub fn api_key_header(
        name: HeaderName,
        key: ApiKey,
        expires_at: Instant,
    ) -> Result<Self, LlmError> {
        validate_auth_name(&name)?;
        Self::new(key, DynamicCredentialScheme::ApiKeyHeader(name), expires_at).map_err(Into::into)
    }

    fn new(
        key: ApiKey,
        scheme: DynamicCredentialScheme,
        expires_at: Instant,
    ) -> Result<Self, CredentialError> {
        if expires_at <= Instant::now() {
            return Err(CredentialError::new(CredentialFailure::Invalid));
        }
        Ok(Self {
            key,
            scheme,
            expires_at,
        })
    }

    /// Returns the declared authentication scheme without exposing the secret.
    pub fn scheme(&self) -> &DynamicCredentialScheme {
        &self.scheme
    }

    /// Returns the monotonic expiration instant.
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    pub(super) fn is_fresh_at(&self, now: Instant, refresh_window: Duration) -> bool {
        self.expires_at
            .checked_duration_since(now)
            .is_some_and(|remaining| remaining > refresh_window)
    }

    fn operation(&self) -> Result<HeaderOperation, LlmError> {
        let (name, value) = match &self.scheme {
            DynamicCredentialScheme::Bearer => (
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.key.expose())),
            ),
            DynamicCredentialScheme::ApiKeyHeader(name) => {
                (name.clone(), HeaderValue::from_str(self.key.expose()))
            }
        };
        let value = value.map_err(|_| CredentialError::new(CredentialFailure::Invalid))?;
        Ok(HeaderOperation::set_sensitive(name, value))
    }
}

impl fmt::Debug for DynamicCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicCredential")
            .field("key", &"[REDACTED]")
            .field("scheme", &self.scheme)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Value-free input supplied to an external dynamic credential callback.
#[derive(Clone)]
pub struct DynamicCredentialContext {
    tenant_id: TenantId,
    provider_id: ProviderId,
    product_id: ProductId,
    audience: CredentialAudience,
    credential_identity: CredentialIdentity,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl DynamicCredentialContext {
    /// Returns the opaque tenant cache partition.
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
    /// Returns the provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }
    /// Returns the exact provider product identifier.
    pub fn product_id(&self) -> &ProductId {
        &self.product_id
    }
    /// Returns the endpoint credential audience.
    pub fn audience(&self) -> &CredentialAudience {
        &self.audience
    }
    /// Returns the opaque credential identity used for cache partitioning.
    pub fn credential_identity(&self) -> &CredentialIdentity {
        &self.credential_identity
    }
    /// Returns the request's absolute monotonic deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    /// Returns the request cancellation handle.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

impl fmt::Debug for DynamicCredentialContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicCredentialContext")
            .field("tenant_id", &"[REDACTED]")
            .field("provider_id", &self.provider_id)
            .field("product_id", &self.product_id)
            .field("audience", &self.audience)
            .field("credential_identity", &"[REDACTED]")
            .field("deadline", &self.deadline)
            .field("is_cancelled", &self.cancellation.is_cancelled())
            .finish()
    }
}

/// Future returned by a dynamic credential callback.
pub type CredentialFuture =
    Pin<Box<dyn Future<Output = Result<DynamicCredential, CredentialError>> + Send + 'static>>;

/// External source for short-lived credentials.
pub trait DynamicCredentialSource: fmt::Debug + Send + Sync {
    /// Acquires or refreshes one complete credential.
    fn acquire(&self, context: DynamicCredentialContext) -> CredentialFuture;
}

/// Dynamic authentication provider with cache, timeout, and lifecycle enforcement.
#[derive(Clone)]
pub struct DynamicAuth {
    source: Arc<dyn DynamicCredentialSource>,
    cache: DynamicCredentialCache,
    tenant_id: TenantId,
    credential_identity: CredentialIdentity,
    audience: CredentialAudience,
    allowed_schemes: Vec<DynamicCredentialScheme>,
    callback_timeout: Duration,
    refresh_window: Duration,
    allow_still_valid_fallback: bool,
}

impl DynamicAuth {
    /// Creates a dynamic Bearer provider with isolated cache identity.
    pub fn new(
        source: Arc<dyn DynamicCredentialSource>,
        audience: CredentialAudience,
        tenant_id: TenantId,
        credential_identity: CredentialIdentity,
    ) -> Self {
        Self {
            source,
            cache: DynamicCredentialCache::new(),
            tenant_id,
            credential_identity,
            audience,
            allowed_schemes: vec![DynamicCredentialScheme::Bearer],
            callback_timeout: Duration::from_secs(5),
            refresh_window: Duration::from_secs(30),
            allow_still_valid_fallback: true,
        }
    }

    /// Shares a cache with other explicitly partitioned dynamic providers.
    #[must_use]
    pub fn with_cache(mut self, cache: DynamicCredentialCache) -> Self {
        self.cache = cache;
        self
    }

    /// Sets the callback timeout. Zero is rejected.
    pub fn with_callback_timeout(mut self, timeout: Duration) -> Result<Self, LlmError> {
        if timeout.is_zero() {
            return Err(ValidationError::new(
                "auth.callback_timeout",
                ValidationReason::OutOfRange,
                "callback timeout must be positive",
            )
            .into());
        }
        self.callback_timeout = timeout;
        Ok(self)
    }

    /// Sets how early a cached credential should be refreshed.
    #[must_use]
    pub fn with_refresh_window(mut self, refresh_window: Duration) -> Self {
        self.refresh_window = refresh_window;
        self
    }

    /// Allows a dynamic source to return one registered API-key header scheme.
    pub fn allow_api_key_header(mut self, name: HeaderName) -> Result<Self, LlmError> {
        validate_auth_name(&name)?;
        let scheme = DynamicCredentialScheme::ApiKeyHeader(name);
        if !self.allowed_schemes.contains(&scheme) {
            self.allowed_schemes.push(scheme);
        }
        Ok(self)
    }

    /// Controls whether a refresh failure may reuse a credential that is not yet expired.
    #[must_use]
    pub fn with_still_valid_fallback(mut self, allowed: bool) -> Self {
        self.allow_still_valid_fallback = allowed;
        self
    }

    async fn resolve_dynamic(
        &self,
        context: AuthContext<'_>,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        self.audience.validate(context.endpoint())?;
        let provider_id = context
            .provider_id()
            .ok_or_else(|| CredentialError::new(CredentialFailure::Invalid))?
            .clone();
        let product_id = context
            .product_id()
            .ok_or_else(|| CredentialError::new(CredentialFailure::Invalid))?
            .clone();
        let lifecycle = context
            .lifecycle()
            .ok_or_else(|| CredentialError::new(CredentialFailure::Invalid))?;
        let key = AuthCacheKey {
            tenant_id: self.tenant_id.clone(),
            provider_id: provider_id.clone(),
            product_id: product_id.clone(),
            audience: self.audience.clone(),
            credential_identity: self.credential_identity.clone(),
        };
        let source = Arc::clone(&self.source);
        let callback_context = DynamicCredentialContext {
            tenant_id: self.tenant_id.clone(),
            provider_id,
            product_id,
            audience: self.audience.clone(),
            credential_identity: self.credential_identity.clone(),
            deadline: lifecycle.deadline(),
            cancellation: lifecycle.cancellation().clone(),
        };
        let allowed_schemes = self.allowed_schemes.clone();
        let callback_timeout = self.callback_timeout;
        let credential = self
            .cache
            .get_or_refresh(
                key,
                self.refresh_window,
                self.allow_still_valid_fallback,
                move || {
                    let source = Arc::clone(&source);
                    let callback_context = callback_context.clone();
                    let allowed_schemes = allowed_schemes.clone();
                    async move {
                        let acquired = await_with_lifecycle(
                            lifecycle,
                            tokio::time::timeout(
                                callback_timeout,
                                source.acquire(callback_context),
                            ),
                        )
                        .await?;
                        let credential = acquired
                            .map_err(|_| CredentialError::new(CredentialFailure::Timeout))??;
                        if !allowed_schemes.contains(credential.scheme())
                            || credential.expires_at() <= Instant::now()
                        {
                            return Err(CredentialError::new(CredentialFailure::Invalid).into());
                        }
                        Ok(credential)
                    }
                },
            )
            .await?;
        Ok(vec![credential.operation()?])
    }
}

impl fmt::Debug for DynamicAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicAuth")
            .field("source", &"<dynamic credential source>")
            .field("cache", &self.cache)
            .field("tenant_id", &"[REDACTED]")
            .field("credential_identity", &"[REDACTED]")
            .field("audience", &self.audience)
            .field("allowed_schemes", &self.allowed_schemes)
            .field("callback_timeout", &self.callback_timeout)
            .field("refresh_window", &self.refresh_window)
            .field(
                "allow_still_valid_fallback",
                &self.allow_still_valid_fallback,
            )
            .finish()
    }
}

impl AuthProvider for DynamicAuth {
    fn resolve<'a>(&'a self, context: AuthContext<'a>) -> AuthFuture<'a> {
        Box::pin(async move { self.resolve_dynamic(context).await })
    }

    fn protected_headers(&self) -> Vec<HeaderName> {
        self.allowed_schemes
            .iter()
            .map(|scheme| match scheme {
                DynamicCredentialScheme::Bearer => header::AUTHORIZATION,
                DynamicCredentialScheme::ApiKeyHeader(name) => name.clone(),
            })
            .collect()
    }

    fn validate_endpoint(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        self.audience.validate(endpoint)
    }

    fn validate_final(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        let populated = self
            .protected_headers()
            .iter()
            .filter(|name| headers.contains_key(*name))
            .count();
        if populated == 1 {
            Ok(())
        } else {
            Err(CredentialError::new(CredentialFailure::Invalid).into())
        }
    }
}
