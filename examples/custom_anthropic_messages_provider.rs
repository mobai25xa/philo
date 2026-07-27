//! Declares a custom Anthropic Messages provider without reading a secret or sending I/O.

use std::error::Error;

use http::HeaderName;
use philo::provider::capability::ProviderCapabilities;
use philo::provider::catalog::ProductId;
use philo::provider::definition::AuthScheme;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::registry::{ProviderRegistration, ProviderRegistry};
use philo::provider::secret::SecretReference;
use philo::{ProviderDefinition, ProviderDeploymentConfig, ProviderId};

fn main() -> Result<(), Box<dyn Error>> {
    let provider = ProviderId::new("example-anthropic-compatible")?;
    let definition =
        ProviderDefinition::anthropic_messages(provider.clone(), ProductId::new("messages")?)
            .with_endpoint(EndpointConfig::absolute(
                "https://messages.example.com/v1/messages",
            )?)
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::api_key_header(HeaderName::from_static(
                "x-api-key",
            ))?)
            .with_anthropic_version("2023-06-01")?
            .with_capabilities(ProviderCapabilities::conservative_messages())
            .allow_unregistered_models()
            .build()?;

    let registry = ProviderRegistry::new();
    registry.register(ProviderRegistration::from_definition(definition)?)?;
    let deployment = ProviderDeploymentConfig::new(
        provider.clone(),
        SecretReference::environment_variable("EXAMPLE_ANTHROPIC_COMPATIBLE_KEY")?,
    );

    assert!(registry.get(&provider)?.is_some());
    assert_eq!(deployment.provider_id(), &provider);
    Ok(())
}
