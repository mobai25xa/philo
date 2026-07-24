#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::sync::Arc;

use http::{HeaderName, HeaderValue};
use url::Url;

use crate::error::{LlmError, ValidationError, ValidationReason};

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::compat::{OpenRouterRoutingContract, OpenRouterRoutingPatch};
use super::super::endpoint::CredentialAudience;
use super::super::headers::{DynamicHeaderPolicy, HeaderOperation};
use super::super::profile::ProviderProfile;
use super::super::runtime::ProviderRuntime;
use super::common::{CompatibleProfileParts, build_compatible_profile, provider_patch};

/// Reviewed `OpenRouter` application attribution headers.
#[derive(Clone)]
pub struct OpenRouterAttribution {
    site_origin: HeaderValue,
    title: HeaderValue,
    categories: Option<HeaderValue>,
}

impl OpenRouterAttribution {
    /// Creates origin-only attribution. Paths, query, fragment, and userinfo are rejected.
    pub fn new(site_origin: &str, title: &str) -> Result<Self, LlmError> {
        let url = Url::parse(site_origin).map_err(|_| invalid("openrouter_attribution.site"))?;
        if url.scheme() != "https"
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(invalid("openrouter_attribution.site"));
        }
        if title.is_empty() || title.len() > 128 || !title.is_ascii() {
            return Err(invalid("openrouter_attribution.title"));
        }
        Ok(Self {
            site_origin: HeaderValue::from_str(site_origin)
                .map_err(|_| invalid("openrouter_attribution.site"))?,
            title: HeaderValue::from_str(title)
                .map_err(|_| invalid("openrouter_attribution.title"))?,
            categories: None,
        })
    }

    /// Adds a bounded comma-separated category list.
    pub fn with_categories<I, S>(mut self, categories: I) -> Result<Self, LlmError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let categories = categories
            .into_iter()
            .map(|value| value.as_ref().to_owned())
            .collect::<Vec<_>>();
        if categories.is_empty()
            || categories.len() > 8
            || categories.iter().any(|value| {
                value.is_empty()
                    || value.len() > 32
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
        {
            return Err(invalid("openrouter_attribution.categories"));
        }
        let joined = categories.join(",");
        self.categories = Some(
            HeaderValue::from_str(&joined)
                .map_err(|_| invalid("openrouter_attribution.categories"))?,
        );
        Ok(self)
    }

    fn operations(&self) -> Vec<HeaderOperation> {
        let mut operations = vec![
            HeaderOperation::set(
                HeaderName::from_static("http-referer"),
                self.site_origin.clone(),
            ),
            HeaderOperation::set(
                HeaderName::from_static("x-openrouter-title"),
                self.title.clone(),
            ),
        ];
        if let Some(categories) = &self.categories {
            operations.push(HeaderOperation::set(
                HeaderName::from_static("x-openrouter-categories"),
                categories.clone(),
            ));
        }
        operations
    }
}

impl fmt::Debug for OpenRouterAttribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterAttribution")
            .field("site_origin", &"[CONFIGURED]")
            .field("title", &"[CONFIGURED]")
            .field("categories_present", &self.categories.is_some())
            .finish()
    }
}

/// Experimental built-in `OpenRouter` Chat Completions preset.
#[derive(Clone, Debug)]
pub struct OpenRouterProfile {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    attribution: Option<OpenRouterAttribution>,
    routing: Option<OpenRouterRoutingContract>,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
}

impl OpenRouterProfile {
    /// Creates the preset with a Bearer credential bound to `OpenRouter` only.
    pub fn new(key: ApiKey) -> Self {
        let credential = BearerCredential::new(key, CredentialAudience::OpenRouterApi);
        Self {
            auth: Arc::new(BearerAuth::new(credential)),
            client_identity: ClientIdentity::default(),
            attribution: None,
            routing: None,
            dynamic_header_policy: None,
        }
    }

    /// Creates the preset from an API key string.
    pub fn from_api_key(key: impl Into<String>) -> Result<Self, LlmError> {
        Ok(Self::new(ApiKey::new(key)?))
    }

    #[must_use]
    /// Replaces the truthful client identity.
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = identity;
        self
    }

    #[must_use]
    /// Adds structured application attribution headers.
    pub fn with_attribution(mut self, attribution: OpenRouterAttribution) -> Self {
        self.attribution = Some(attribution);
        self
    }

    #[must_use]
    /// Installs typed `OpenRouter` gateway routing defaults.
    pub fn with_routing(mut self, patch: OpenRouterRoutingPatch) -> Self {
        self.routing = Some(OpenRouterRoutingContract::new(patch));
        self
    }

    #[must_use]
    /// Replaces Bearer authentication with an extensible provider.
    pub fn with_auth_provider<A: AuthProvider + 'static>(mut self, auth: A) -> Self {
        self.auth = Arc::new(auth);
        self
    }

    #[must_use]
    /// Installs a controlled value-free dynamic header policy.
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.dynamic_header_policy = Some(Arc::new(policy));
        self
    }

    /// Produces the declarative profile.
    pub fn profile(self) -> Result<ProviderProfile, LlmError> {
        let provider_headers = self
            .attribution
            .as_ref()
            .map_or_else(Vec::new, OpenRouterAttribution::operations);
        build_compatible_profile(CompatibleProfileParts {
            provider: "openrouter",
            product: "openrouter-chat",
            base_url: "https://openrouter.ai/api/v1",
            endpoint_path: "/chat/completions",
            audience: CredentialAudience::OpenRouterApi,
            auth: self.auth,
            client_identity: self.client_identity,
            provider_headers,
            dynamic_header_policy: self.dynamic_header_policy,
            exact_model: "nvidia/nemotron-3-ultra-550b-a55b:free",
            display_name: "OpenRouter NVIDIA Nemotron 3 Ultra 550B A55B (free)",
            catalog_source: "p3-001-openrouter-official-docs",
            provider_compat: provider_patch(),
            openrouter_routing: self.routing,
        })
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}

fn invalid(field: &'static str) -> LlmError {
    ValidationError::new(
        field,
        ValidationReason::InvalidIdentifier,
        "OpenRouter attribution value is outside the safe structured subset",
    )
    .into()
}
