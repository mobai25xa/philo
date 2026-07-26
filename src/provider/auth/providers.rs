//! Static authentication provider shapes.

use std::collections::HashSet;
use std::fmt;

use http::{HeaderMap, HeaderName, HeaderValue, header};

use super::{ApiKey, AuthContext, AuthFuture, AuthProvider, AuthSchemeKind, CredentialSourceKind};
use crate::error::{LlmError, ValidationError, ValidationReason};
use crate::provider::endpoint::{CredentialBinding, ResolvedEndpoint};
use crate::provider::headers::HeaderOperation;

/// Fixed API-key value written to one explicitly registered authentication header.
#[derive(Clone)]
pub struct ApiKeyHeaderAuth {
    name: HeaderName,
    key: ApiKey,
    binding: CredentialBinding,
}

impl ApiKeyHeaderAuth {
    /// Creates a fixed API-key authentication provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected header belongs to protocol, transport,
    /// identity, or cookie handling.
    pub fn new(
        name: HeaderName,
        key: ApiKey,
        binding: impl Into<CredentialBinding>,
    ) -> Result<Self, LlmError> {
        validate_auth_name(&name)?;
        Ok(Self {
            name,
            key,
            binding: binding.into(),
        })
    }

    fn operation(&self, context: AuthContext<'_>) -> Result<HeaderOperation, LlmError> {
        self.binding.validate(context.endpoint())?;
        let value = HeaderValue::from_str(self.key.expose()).map_err(|_| invalid_auth_value())?;
        Ok(HeaderOperation::set_sensitive(self.name.clone(), value))
    }
}

impl fmt::Debug for ApiKeyHeaderAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiKeyHeaderAuth")
            .field("name", &self.name)
            .field("key", &"[REDACTED]")
            .field("binding", &self.binding)
            .finish()
    }
}

impl AuthProvider for ApiKeyHeaderAuth {
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
        vec![self.name.clone()]
    }

    fn credential_binding(&self) -> Option<&CredentialBinding> {
        Some(&self.binding)
    }

    fn validate_endpoint(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        self.binding.validate(endpoint)
    }

    fn validate_final(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        if headers.contains_key(&self.name) {
            Ok(())
        } else {
            Err(protected_header_error(
                "API-key auth provider must set its registered header",
            ))
        }
    }

    fn scheme_kind(&self) -> AuthSchemeKind {
        AuthSchemeKind::ApiKeyHeader
    }

    fn credential_source_kind(&self) -> CredentialSourceKind {
        CredentialSourceKind::Static
    }
}

/// Atomic fixed group of authentication headers.
#[derive(Clone)]
pub struct MultiHeaderAuth {
    entries: Vec<(HeaderName, ApiKey)>,
    binding: CredentialBinding,
}

impl MultiHeaderAuth {
    /// Creates a non-empty, duplicate-free authentication group.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty group, duplicate names, forbidden names, or
    /// more than eight authentication headers.
    pub fn new(
        entries: Vec<(HeaderName, ApiKey)>,
        binding: impl Into<CredentialBinding>,
    ) -> Result<Self, LlmError> {
        if entries.is_empty() || entries.len() > 8 {
            return Err(ValidationError::new(
                "auth.multi_header",
                ValidationReason::OutOfRange,
                "multi-header auth requires one to eight entries",
            )
            .into());
        }
        let mut names = HashSet::with_capacity(entries.len());
        for (name, _) in &entries {
            validate_auth_name(name)?;
            if !names.insert(name.clone()) {
                return Err(ValidationError::new(
                    "auth.multi_header",
                    ValidationReason::InvalidHeader,
                    "multi-header auth contains a duplicate name",
                )
                .into());
            }
        }
        Ok(Self {
            entries,
            binding: binding.into(),
        })
    }

    fn operations(&self, context: AuthContext<'_>) -> Result<Vec<HeaderOperation>, LlmError> {
        self.binding.validate(context.endpoint())?;
        self.entries
            .iter()
            .map(|(name, key)| {
                HeaderValue::from_str(key.expose())
                    .map(|value| HeaderOperation::set_sensitive(name.clone(), value))
                    .map_err(|_| invalid_auth_value())
            })
            .collect()
    }
}

impl fmt::Debug for MultiHeaderAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self
            .entries
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        formatter
            .debug_struct("MultiHeaderAuth")
            .field("names", &names)
            .field("values", &"[REDACTED]")
            .field("binding", &self.binding)
            .finish()
    }
}

impl AuthProvider for MultiHeaderAuth {
    fn resolve<'a>(&'a self, context: AuthContext<'a>) -> AuthFuture<'a> {
        Box::pin(async move { self.operations(context) })
    }

    fn resolve_immediate(
        &self,
        context: AuthContext<'_>,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        self.operations(context)
    }

    fn protected_headers(&self) -> Vec<HeaderName> {
        self.entries.iter().map(|(name, _)| name.clone()).collect()
    }

    fn credential_binding(&self) -> Option<&CredentialBinding> {
        Some(&self.binding)
    }

    fn validate_endpoint(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        self.binding.validate(endpoint)
    }

    fn validate_final(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        if self
            .entries
            .iter()
            .all(|(name, _)| headers.contains_key(name))
        {
            Ok(())
        } else {
            Err(protected_header_error(
                "multi-header auth provider must set its complete group",
            ))
        }
    }

    fn scheme_kind(&self) -> AuthSchemeKind {
        AuthSchemeKind::MultiHeader
    }

    fn credential_source_kind(&self) -> CredentialSourceKind {
        CredentialSourceKind::Static
    }
}

/// Explicit unauthenticated provider. Empty keys never imply this mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAuth;

impl AuthProvider for NoAuth {
    fn resolve<'a>(&'a self, _context: AuthContext<'a>) -> AuthFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn resolve_immediate(
        &self,
        _context: AuthContext<'_>,
    ) -> Result<Vec<HeaderOperation>, LlmError> {
        Ok(Vec::new())
    }

    fn protected_headers(&self) -> Vec<HeaderName> {
        Vec::new()
    }

    fn validate_endpoint(&self, _endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        Ok(())
    }

    fn validate_final(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        if headers.contains_key(header::AUTHORIZATION)
            || headers.contains_key(header::PROXY_AUTHORIZATION)
        {
            Err(protected_header_error(
                "NoAuth runtime must not contain authorization headers",
            ))
        } else {
            Ok(())
        }
    }

    fn scheme_kind(&self) -> AuthSchemeKind {
        AuthSchemeKind::None
    }

    fn credential_source_kind(&self) -> CredentialSourceKind {
        CredentialSourceKind::None
    }
}

pub(super) fn validate_auth_name(name: &HeaderName) -> Result<(), LlmError> {
    if name.as_str().len() > 128
        || matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "content-type"
                | "accept"
                | "transfer-encoding"
                | "connection"
                | "cookie"
                | "set-cookie"
                | "user-agent"
        )
    {
        Err(ValidationError::new(
            "auth.header_name",
            ValidationReason::ProtectedHeader,
            "header belongs to a non-authentication owner",
        )
        .into())
    } else {
        Ok(())
    }
}

fn invalid_auth_value() -> LlmError {
    ValidationError::new(
        "auth.header_value",
        ValidationReason::InvalidHeader,
        "credential cannot form an HTTP header value",
    )
    .into()
}

fn protected_header_error(summary: &'static str) -> LlmError {
    ValidationError::new(
        "request_headers.auth",
        ValidationReason::ProtectedHeader,
        summary,
    )
    .into()
}
