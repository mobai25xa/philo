//! Compiles an external configuration snapshot into the core's two construction inputs.

use std::error::Error;

use philo::ProviderRuntime;
use philo::provider::secret::{EnvironmentSecretResolver, SecretReference};
use philo_config::{ConfigSource, ConfigValue, ProviderConfigLayer, ProviderConfigSnapshot};

fn main() -> Result<(), Box<dyn Error>> {
    let credential = ProviderConfigLayer::new(ConfigSource::environment_secret(
        "env/official-openai-key",
        "OPENAI_API_KEY",
    )?)
    .with_credential(ConfigValue::set(SecretReference::environment_variable(
        "OPENAI_API_KEY",
    )?));
    let snapshot = ProviderConfigSnapshot::official_openai()?.merge_layers([credential])?;
    let (definition, deployment) = snapshot.official_openai_inputs()?;

    if std::env::var_os("OPENAI_API_KEY").is_some() {
        let profile = definition.compile(&deployment, &EnvironmentSecretResolver)?;
        let runtime = ProviderRuntime::build(profile)?;
        println!("provider={}", runtime.provider_id());
    } else {
        println!("definition and deployment produced; credential was not resolved");
    }
    Ok(())
}
