//! Production-path provider builder used only by repository tests and harnesses.
#![allow(dead_code, unreachable_pub)]

use std::sync::Arc;

use http::{HeaderName, HeaderValue};
use philo::domain::{ModelId, ProviderId};
use philo::error::{LlmError, ProviderConfigError, ProviderConfigFailure};
use philo::provider::auth::{ApiKey, AuthProvider};
use philo::provider::endpoint::EndpointConfig;
use philo::provider::factory::StaticProviderFactory;
use philo::provider::headers::{DynamicHeaderPolicy, HeaderOperation};
use philo::provider::profiles::OFFICIAL_ANTHROPIC_API_VERSION;
use philo::provider::secret::{SecretReference, SecretResolver};
use philo::provider::{
    AuthScheme, CompatProfile, IdempotencyPolicy, ModelCapabilityProfile, ModelCatalog, ProductId,
    ProviderCapabilities, ProviderDefinition, ProviderDefinitionBuilder, ProviderDeploymentConfig,
    ProviderRuntime, RateLimitPolicy,
};

#[derive(Clone, Copy)]
enum Protocol {
    OpenAiChat,
    AnthropicMessages,
}

/// Test-only wrapper around the same public definition/deployment path used by consumers.
pub struct TestProvider {
    endpoint: EndpointConfig,
    key: String,
    protocol: Protocol,
    model_capabilities: Vec<ModelCapabilityProfile>,
    catalog: Option<ModelCatalog>,
    compat: Option<CompatProfile>,
    model_compat: Vec<(ModelId, CompatProfile)>,
    auth: Option<Arc<dyn AuthProvider>>,
    dynamic_header_policy: Option<DynamicHeaderPolicy>,
    rate_limit: RateLimitPolicy,
    idempotency: IdempotencyPolicy,
}

impl TestProvider {
    pub fn new(endpoint: &str, key: impl Into<String>) -> Result<Self, LlmError> {
        Ok(Self {
            endpoint: EndpointConfig::absolute(endpoint)?,
            key: key.into(),
            protocol: Protocol::OpenAiChat,
            model_capabilities: Vec::new(),
            catalog: None,
            compat: None,
            model_compat: Vec::new(),
            auth: None,
            dynamic_header_policy: None,
            rate_limit: RateLimitPolicy::standard_only(),
            idempotency: IdempotencyPolicy::standard_header(),
        })
    }

    #[must_use]
    pub fn with_model_capabilities(mut self, profile: ModelCapabilityProfile) -> Self {
        self.model_capabilities.push(profile);
        self
    }

    #[must_use]
    pub fn with_anthropic_messages(mut self) -> Self {
        self.protocol = Protocol::AnthropicMessages;
        self
    }

    #[must_use]
    pub fn with_auth_provider<A>(mut self, auth: A) -> Self
    where
        A: AuthProvider + 'static,
    {
        self.auth = Some(Arc::new(auth));
        self
    }

    #[must_use]
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.dynamic_header_policy = Some(policy);
        self
    }

    #[must_use]
    pub fn with_endpoint_config(mut self, endpoint: EndpointConfig) -> Self {
        self.endpoint = endpoint;
        self
    }

    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    #[must_use]
    pub fn with_compat(mut self, compat: CompatProfile) -> Self {
        self.compat = Some(compat);
        self
    }

    #[must_use]
    pub fn with_model_compat(mut self, model: ModelId, compat: CompatProfile) -> Self {
        self.model_compat.push((model, compat));
        self
    }

    #[must_use]
    pub fn with_rate_limit_policy(mut self, policy: RateLimitPolicy) -> Self {
        self.rate_limit = policy;
        self
    }

    #[must_use]
    pub fn with_idempotency_policy(mut self, policy: IdempotencyPolicy) -> Self {
        self.idempotency = policy;
        self
    }

    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        let provider_id = ProviderId::new("test-only")?;
        let product_id = match self.protocol {
            Protocol::OpenAiChat => ProductId::new("chat-completions")?,
            Protocol::AnthropicMessages => ProductId::new("messages")?,
        };
        let mut builder = match self.protocol {
            Protocol::OpenAiChat => {
                ProviderDefinition::openai_chat(provider_id.clone(), product_id)
                    .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            }
            Protocol::AnthropicMessages => {
                ProviderDefinition::anthropic_messages(provider_id.clone(), product_id)
                    .with_capabilities(ProviderCapabilities::conservative_messages())
                    .with_provider_headers(vec![HeaderOperation::set(
                        HeaderName::from_static("anthropic-version"),
                        HeaderValue::from_static(OFFICIAL_ANTHROPIC_API_VERSION),
                    )])
            }
        };
        builder = builder
            .with_endpoint(self.endpoint)
            .bind_credential_to_endpoint_origin()
            .with_rate_limit_policy(self.rate_limit)
            .with_idempotency_policy(self.idempotency);
        builder = add_auth_scheme(builder, self.auth.as_deref())?;
        builder = if let Some(catalog) = self.catalog {
            builder.with_catalog(catalog)
        } else {
            builder.allow_unregistered_models()
        };
        for profile in self.model_capabilities {
            builder = builder.with_model_capabilities(profile);
        }
        if let Some(policy) = self.dynamic_header_policy {
            builder = builder.with_dynamic_header_policy(policy);
        }
        if let Some(compat) = self.compat {
            builder = builder.with_openai_chat_compat(compat);
        }
        for (model, compat) in self.model_compat {
            builder = builder.with_model_openai_chat_compat(model, compat);
        }
        let definition = builder.build()?;
        let deployment = if let Some(auth) = self.auth {
            ProviderDeploymentConfig::with_auth_provider(provider_id, auth)
        } else {
            ProviderDeploymentConfig::new(
                provider_id,
                SecretReference::environment_variable("PHILO_TEST_CREDENTIAL")?,
            )
        };
        StaticProviderFactory::new(definition)
            .build_deployment(&deployment, &InlineSecret(self.key))
    }
}

fn add_auth_scheme(
    builder: ProviderDefinitionBuilder,
    auth: Option<&dyn AuthProvider>,
) -> Result<ProviderDefinitionBuilder, LlmError> {
    let scheme = auth.map_or_else(|| Ok(AuthScheme::bearer()), AuthScheme::from_auth_provider)?;
    Ok(builder.with_auth_scheme(scheme))
}

struct InlineSecret(String);

impl SecretResolver for InlineSecret {
    fn resolve(&self, _reference: &SecretReference) -> Result<ApiKey, ProviderConfigError> {
        ApiKey::new(self.0.clone()).map_err(|_| {
            ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::InvalidValue,
                "test credential is invalid",
            )
        })
    }
}
