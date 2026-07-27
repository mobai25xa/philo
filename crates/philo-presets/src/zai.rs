#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::sync::Arc;

use http::{HeaderName, HeaderValue};

use super::common::{PRESET_SOURCE, compatible_deployment, exact_model_catalog, provider_contract};
use philo::domain::{ModelId, ProtocolId, ProviderId};
use philo::error::{LlmError, ValidationError, ValidationReason};
use philo::provider::EnvironmentSecretResolver;
use philo::provider::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use philo::provider::capability::ProviderCapabilities;
use philo::provider::catalog::ProductId;
use philo::provider::definition::{AuthScheme, ProviderDefinition};
use philo::provider::endpoint::{CredentialAudience, EndpointConfig};
use philo::provider::headers::{DynamicHeaderPolicy, HeaderOperation};
use philo::provider::profile::ProviderProfile;
use philo::provider::protocol_contract::MaxOutputTokensWireFormat;
use philo::provider::runtime::ProviderRuntime;

fn language_operation(value: Option<HeaderValue>) -> Vec<HeaderOperation> {
    value.map_or_else(Vec::new, |value| {
        vec![HeaderOperation::set(
            HeaderName::from_static("accept-language"),
            value,
        )]
    })
}

fn parse_language(value: &str) -> Result<HeaderValue, LlmError> {
    if value.is_empty()
        || value.len() > 35
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ValidationError::new(
            "zai.accept_language",
            ValidationReason::InvalidIdentifier,
            "Z.AI language must be one bounded language tag",
        )
        .into());
    }
    HeaderValue::from_str(value).map_err(|_| {
        ValidationError::new(
            "zai.accept_language",
            ValidationReason::InvalidHeader,
            "Z.AI language cannot form a header",
        )
        .into()
    })
}

macro_rules! zai_profile {
    (
        $name:ident,
        $docs:literal,
        $product:literal,
        $base:literal,
        $audience:expr,
        $model:literal,
        $display_name:literal,
        $source:literal
    ) => {
        #[doc = $docs]
        #[derive(Clone, Debug)]
        pub struct $name {
            auth: Arc<dyn AuthProvider>,
            client_identity: ClientIdentity,
            accept_language: Option<HeaderValue>,
            dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
        }

        impl $name {
            /// Creates the preset with a product-scoped Bearer credential.
            pub fn new(key: ApiKey) -> Self {
                let credential = BearerCredential::new(key, $audience);
                Self {
                    auth: Arc::new(BearerAuth::new(credential)),
                    client_identity: ClientIdentity::default(),
                    accept_language: None,
                    dynamic_header_policy: None,
                }
            }

            /// Creates the preset directly from an API key string.
            pub fn from_api_key(key: impl Into<String>) -> Result<Self, LlmError> {
                Ok(Self::new(ApiKey::new(key)?))
            }

            #[must_use]
            /// Replaces the truthful client identity.
            pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
                self.client_identity = identity;
                self
            }

            /// Adds one structured `Accept-Language` tag.
            pub fn with_accept_language(mut self, value: &str) -> Result<Self, LlmError> {
                self.accept_language = Some(parse_language(value)?);
                Ok(self)
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
                let compat = provider_contract()
                    .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens, PRESET_SOURCE);
                let provider_id = ProviderId::new("zai")?;
                let product_id = ProductId::new($product)?;
                let protocol_id = ProtocolId::new("openai-chat-completions")?;
                let model_id = ModelId::new($model)?;
                let catalog = exact_model_catalog(
                    provider_id.clone(),
                    product_id.clone(),
                    protocol_id,
                    &model_id,
                    $display_name,
                    $source,
                )?;
                let auth_scheme = AuthScheme::from_auth_provider(self.auth.as_ref())?;
                let mut builder = ProviderDefinition::openai_chat(provider_id.clone(), product_id)
                    .with_endpoint(EndpointConfig::base_and_path($base, "/chat/completions")?)
                    .with_credential_binding($audience.into())
                    .with_auth_scheme(auth_scheme)
                    .with_provider_headers(language_operation(self.accept_language))
                    .with_capabilities(ProviderCapabilities::conservative_chat_completions())
                    .with_catalog(catalog)
                    .with_openai_chat_compat(compat);
                if let Some(policy) = self.dynamic_header_policy {
                    builder = builder.with_dynamic_header_policy(Arc::unwrap_or_clone(policy));
                }
                let definition = builder.build()?;
                let deployment =
                    compatible_deployment(provider_id, self.auth, self.client_identity);
                definition.compile(&deployment, &EnvironmentSecretResolver)
            }

            /// Builds the immutable runtime.
            pub fn build(self) -> Result<ProviderRuntime, LlmError> {
                ProviderRuntime::build(self.profile()?)
            }
        }
    };
}

zai_profile!(
    ZaiStandardProfile,
    "Experimental built-in Z.AI Standard `PaaS` Chat preset.",
    "zai-standard-api",
    "https://api.z.ai/api/paas/v4",
    CredentialAudience::ZaiStandard,
    "glm-4.7-flash",
    "Z.AI GLM-4.7-Flash",
    "p3-001-zai-standard-official-docs"
);

zai_profile!(
    ZaiCodingProfile,
    "Experimental built-in Z.AI Coding Plan Chat preset with an isolated audience.",
    "zai-coding-plan",
    "https://api.z.ai/api/coding/paas/v4",
    CredentialAudience::ZaiCoding,
    "glm-4.7-flash",
    "Z.AI GLM-4.7-Flash",
    "p3-001-zai-coding-official-docs"
);
