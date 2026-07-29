//! Stable consumer for explicit custom-provider definitions.

use philo::provider::catalog::ProductId;
use philo::provider::definition::{AuthScheme, ProviderDefinition};
use philo::provider::endpoint::EndpointConfig;
use philo::{LlmError, ProviderId};

/// Builds a secret-free custom provider definition.
pub fn definition() -> Result<ProviderDefinition, LlmError> {
    ProviderDefinition::openai_chat(
        ProviderId::new("consumer-provider")?,
        ProductId::new("chat")?,
    )
    .with_endpoint(EndpointConfig::absolute(
        "https://llm.example.com/v1/chat/completions",
    )?)
    .bind_credential_to_endpoint_origin()
    .with_auth_scheme(AuthScheme::bearer())
    .allow_unregistered_models()
    .build()
}
