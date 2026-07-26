//! Declares a custom `OpenAI` Chat Completions provider without reading a secret or sending I/O.

use std::error::Error;

use philo::{
    AuthScheme, EndpointConfig, ProductId, ProviderCapabilities, ProviderDefinition,
    ProviderDeploymentConfig, ProviderId, ProviderRegistration, ProviderRegistry, SecretReference,
};

fn main() -> Result<(), Box<dyn Error>> {
    let provider = ProviderId::new("example-openai-compatible")?;
    let definition =
        ProviderDefinition::openai_chat(provider.clone(), ProductId::new("chat-completions")?)
            .with_endpoint(EndpointConfig::absolute(
                "https://llm.example.com/v1/chat/completions",
            )?)
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            .allow_unregistered_models()
            .build()?;

    let registry = ProviderRegistry::new();
    registry.register(ProviderRegistration::from_definition(definition)?)?;
    let deployment = ProviderDeploymentConfig::new(
        provider.clone(),
        SecretReference::environment_variable("EXAMPLE_OPENAI_COMPATIBLE_KEY")?,
    );

    assert!(registry.get(&provider)?.is_some());
    assert_eq!(deployment.provider_id(), &provider);
    Ok(())
}
