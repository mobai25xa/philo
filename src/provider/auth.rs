//! Secret handling, Bearer authentication, and truthful client identity.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use http::{HeaderValue, header};
use secrecy::{ExposeSecret, SecretString};

use super::endpoint::{CredentialAudience, ResolvedEndpoint};
use super::headers::HeaderOperation;
use crate::error::{LlmError, ValidationError, ValidationReason};

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

/// Bearer credential bound to a credential audience.
#[derive(Clone)]
pub struct BearerCredential {
    key: ApiKey,
    audience: CredentialAudience,
}

impl BearerCredential {
    /// Binds a secret to its allowed destination.
    pub fn new(key: ApiKey, audience: CredentialAudience) -> Self {
        Self { key, audience }
    }
    /// Returns the credential audience.
    pub fn audience(&self) -> &CredentialAudience {
        &self.audience
    }
}

impl fmt::Debug for BearerCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerCredential")
            .field("key", &"[REDACTED]")
            .field("audience", &self.audience)
            .finish()
    }
}

/// Safe authentication context containing no prompt or wire request.
#[derive(Clone, Copy, Debug)]
pub struct AuthContext<'a> {
    endpoint: &'a ResolvedEndpoint,
}

impl<'a> AuthContext<'a> {
    /// Creates an authentication context for a resolved endpoint.
    pub fn new(endpoint: &'a ResolvedEndpoint) -> Self {
        Self { endpoint }
    }
    /// Returns the final endpoint.
    pub fn endpoint(&self) -> &'a ResolvedEndpoint {
        self.endpoint
    }
}

/// Authentication source that contributes only protected auth headers.
pub trait AuthProvider: fmt::Debug + Send + Sync {
    /// Produces the authentication header after validating credential audience.
    fn operation(&self, context: AuthContext<'_>) -> Result<HeaderOperation, LlmError>;
}

/// Phase-one Bearer authentication provider.
#[derive(Clone)]
pub struct BearerAuth {
    credential: BearerCredential,
}

impl BearerAuth {
    /// Creates a Bearer authentication provider.
    pub fn new(credential: BearerCredential) -> Self {
        Self { credential }
    }
    /// Returns the bound audience.
    pub fn audience(&self) -> &CredentialAudience {
        self.credential.audience()
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
    fn operation(&self, context: AuthContext<'_>) -> Result<HeaderOperation, LlmError> {
        self.credential.audience.validate(context.endpoint())?;
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

/// Truthful User-Agent identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientIdentity {
    product: String,
    version: String,
}

impl ClientIdentity {
    /// Creates a controlled product identity.
    pub fn new(product: impl Into<String>, version: impl Into<String>) -> Result<Self, LlmError> {
        let product = product.into();
        let version = version.into();
        if !valid_identity_token(&product) || !valid_identity_token(&version) {
            return Err(validation(
                "client_identity",
                ValidationReason::InvalidHeader,
                "product and version must be non-empty ASCII tokens",
            ));
        }
        if product.to_ascii_lowercase().contains("openai") {
            return Err(validation(
                "client_identity.product",
                ValidationReason::OutOfRange,
                "identity must not impersonate an OpenAI SDK",
            ));
        }
        let identity = Self { product, version };
        identity.header_value()?;
        Ok(identity)
    }
    /// Returns the default `philo/<crate-version>` identity.
    pub fn philo() -> Self {
        Self {
            product: crate::SDK_NAME.to_owned(),
            version: crate::SDK_VERSION.to_owned(),
        }
    }
    /// Returns product name.
    pub fn product(&self) -> &str {
        &self.product
    }
    /// Returns product version.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Produces a User-Agent header operation.
    pub fn operation(&self) -> Result<HeaderOperation, LlmError> {
        Ok(HeaderOperation::set(
            header::USER_AGENT,
            self.header_value()?,
        ))
    }

    fn header_value(&self) -> Result<HeaderValue, LlmError> {
        HeaderValue::from_str(&format!("{}/{}", self.product, self.version)).map_err(|_| {
            validation(
                "user-agent",
                ValidationReason::InvalidHeader,
                "client identity is not a valid User-Agent",
            )
        })
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::philo()
    }
}

fn valid_identity_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn validation(
    field: impl Into<String>,
    reason: ValidationReason,
    summary: &'static str,
) -> LlmError {
    ValidationError::new(field, reason, summary).into()
}
