//! Registers two providers that independently reuse `OpenAI` Chat Completions.

use std::error::Error;

use philo::{
    AuthScheme, EndpointConfig, ProductId, ProviderCapabilities, ProviderDefinition, ProviderId,
    ProviderRegistration, ProviderRegistry,
};

fn definition(provider: &str, endpoint: &str) -> Result<ProviderDefinition, philo::LlmError> {
    ProviderDefinition::openai_chat(ProviderId::new(provider)?, ProductId::new("chat")?)
        .with_endpoint(EndpointConfig::absolute(endpoint)?)
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::bearer())
        .with_capabilities(ProviderCapabilities::conservative_chat_completions())
        .allow_unregistered_models()
        .build()
}

fn main() -> Result<(), Box<dyn Error>> {
    let registry = ProviderRegistry::new();
    registry.register(ProviderRegistration::from_definition(definition(
        "example-provider-a",
        "https://a.example.com/v1/chat/completions",
    )?)?)?;
    registry.register(ProviderRegistration::from_definition(definition(
        "example-provider-b",
        "https://b.example.com/v1/chat/completions",
    )?)?)?;

    assert_eq!(registry.list()?.len(), 2);
    Ok(())
}
