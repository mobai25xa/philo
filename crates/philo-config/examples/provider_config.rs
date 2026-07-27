//! Merges versioned provider configuration while retaining value-free provenance.

use std::error::Error;

use philo::provider::secret::SecretReference;
use philo_config::{
    ClientIdentityConfig, ConfigSource, ConfigValue, ProviderConfigField, ProviderConfigLayer,
    ProviderConfigSnapshot,
};

fn main() -> Result<(), Box<dyn Error>> {
    let user = ProviderConfigLayer::new(ConfigSource::user_config(
        "file/provider",
        "providers.json",
    )?)
    .with_client_identity(ConfigValue::set(ClientIdentityConfig::new(
        "my-application",
        "1.0.0",
    )));
    let secret = ProviderConfigLayer::new(ConfigSource::environment_secret(
        "env/provider-key",
        "OPENAI_API_KEY",
    )?)
    .with_credential(ConfigValue::set(SecretReference::environment_variable(
        "OPENAI_API_KEY",
    )?));
    let snapshot = ProviderConfigSnapshot::official_openai()?.merge_layers([user, secret])?;

    let source = snapshot
        .provenance(ProviderConfigField::Credential)
        .expect("credential source");
    println!(
        "schema={:?} credential_source={} state={:?}",
        snapshot.version(),
        source.source().id(),
        source.state()
    );
    Ok(())
}
