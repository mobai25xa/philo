//! Builds a custom HTTPS OpenAI-compatible provider through the public definition path.

use std::error::Error;

use philo::provider::capability::ProviderCapabilities;
use philo::provider::catalog::ProductId;
use philo::provider::definition::AuthScheme;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::secret::{EnvironmentSecretResolver, SecretReference};
use philo::{ProviderDefinition, ProviderDeploymentConfig, ProviderId, ProviderRuntime};

fn main() -> Result<(), Box<dyn Error>> {
    let provider_id = ProviderId::new("example-openai-compatible")?;
    let definition =
        ProviderDefinition::openai_chat(provider_id.clone(), ProductId::new("chat-completions")?)
            .with_endpoint(EndpointConfig::absolute(
                "https://llm.example.com/v1/chat/completions",
            )?)
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            .allow_unregistered_models()
            .build()?;
    let deployment = ProviderDeploymentConfig::new(
        provider_id,
        SecretReference::environment_variable("EXAMPLE_OPENAI_COMPATIBLE_KEY")?,
    );

    if std::env::var_os("EXAMPLE_OPENAI_COMPATIBLE_KEY").is_some() {
        let profile = definition.compile(&deployment, &EnvironmentSecretResolver)?;
        let runtime = ProviderRuntime::build(profile)?;
        println!("provider={}", runtime.provider_id());
    } else {
        println!("definition and deployment validated; credential was not resolved");
    }
    Ok(())
}
