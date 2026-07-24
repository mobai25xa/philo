//! Structured, truthful client identity headers.

use http::{HeaderValue, header};

use super::HeaderOperation;
use crate::error::{LlmError, ValidationError, ValidationReason};

/// One optional application fragment in a User-Agent value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientIdentityFragment {
    name: String,
    version: Option<String>,
}

impl ClientIdentityFragment {
    /// Creates a validated fragment.
    pub fn new(name: impl Into<String>, version: Option<String>) -> Result<Self, LlmError> {
        let name = name.into();
        validate_token("client_identity.fragment.name", &name)?;
        if let Some(version) = &version {
            validate_token("client_identity.fragment.version", version)?;
        }
        Ok(Self { name, version })
    }
}

/// Structured SDK/application identity used to build User-Agent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientIdentity {
    product: String,
    version: String,
    contact: Option<String>,
    application: Option<ClientIdentityFragment>,
    fragments: Vec<ClientIdentityFragment>,
}

impl ClientIdentity {
    /// Creates a controlled product identity.
    pub fn new(product: impl Into<String>, version: impl Into<String>) -> Result<Self, LlmError> {
        let product = product.into();
        let version = version.into();
        validate_token("client_identity.product", &product)?;
        validate_token("client_identity.version", &version)?;
        reject_impersonation(&product)?;
        let identity = Self {
            product,
            version,
            contact: None,
            application: None,
            fragments: Vec::new(),
        };
        identity.header_value()?;
        Ok(identity)
    }

    /// Returns the default `philo/<crate-version>` identity.
    pub fn philo() -> Self {
        Self {
            product: crate::SDK_NAME.to_owned(),
            version: crate::SDK_VERSION.to_owned(),
            contact: None,
            application: None,
            fragments: Vec::new(),
        }
    }

    /// Adds a contact token rendered as a parenthesized User-Agent comment.
    pub fn with_contact(mut self, contact: impl Into<String>) -> Result<Self, LlmError> {
        let contact = contact.into();
        if contact.is_empty()
            || contact.len() > 128
            || !contact
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b'(' && byte != b')')
        {
            return Err(invalid_identity("contact must be a bounded visible token"));
        }
        self.contact = Some(contact);
        self.header_value()?;
        Ok(self)
    }

    /// Adds an application attribution fragment.
    pub fn with_application(
        mut self,
        application: ClientIdentityFragment,
    ) -> Result<Self, LlmError> {
        reject_impersonation(&application.name)?;
        self.application = Some(application);
        self.header_value()?;
        Ok(self)
    }

    /// Adds a non-sensitive attribution fragment.
    pub fn with_fragment(mut self, fragment: ClientIdentityFragment) -> Result<Self, LlmError> {
        reject_impersonation(&fragment.name)?;
        if self.fragments.len() >= 4 {
            return Err(invalid_identity(
                "at most four identity fragments are allowed",
            ));
        }
        self.fragments.push(fragment);
        self.header_value()?;
        Ok(self)
    }

    /// Returns product name.
    pub fn product(&self) -> &str {
        &self.product
    }
    /// Returns product version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Produces a User-Agent operation with no sensitive value.
    pub fn operation(&self) -> Result<HeaderOperation, LlmError> {
        Ok(HeaderOperation::set(
            header::USER_AGENT,
            self.header_value()?,
        ))
    }

    fn header_value(&self) -> Result<HeaderValue, LlmError> {
        let mut parts = vec![format!("{}/{}", self.product, self.version)];
        if let Some(application) = &self.application {
            parts.push(format_fragment(application));
        }
        parts.extend(self.fragments.iter().map(format_fragment));
        let mut value = parts.join(" ");
        if let Some(contact) = &self.contact {
            value.push_str(" (");
            value.push_str(contact);
            value.push(')');
        }
        if value.len() > 512 {
            return Err(invalid_identity("User-Agent exceeds 512 bytes"));
        }
        HeaderValue::from_str(&value)
            .map_err(|_| invalid_identity("client identity is not a valid User-Agent"))
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::philo()
    }
}

fn format_fragment(fragment: &ClientIdentityFragment) -> String {
    fragment.version.as_ref().map_or_else(
        || fragment.name.clone(),
        |version| format!("{}/{}", fragment.name, version),
    )
}

fn validate_token(field: &'static str, value: &str) -> Result<(), LlmError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ValidationError::new(
            field,
            ValidationReason::InvalidHeader,
            "identity values must be bounded ASCII tokens",
        )
        .into());
    }
    Ok(())
}

fn reject_impersonation(value: &str) -> Result<(), LlmError> {
    let lower = value.to_ascii_lowercase();
    if ["openai", "mozilla", "chrome", "safari", "firefox", "curl"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return Err(invalid_identity(
            "identity must not impersonate an official SDK or browser",
        ));
    }
    Ok(())
}

fn invalid_identity(summary: &'static str) -> LlmError {
    ValidationError::new("client_identity", ValidationReason::InvalidHeader, summary).into()
}
