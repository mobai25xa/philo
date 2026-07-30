//! Experimental consumer for versioned external configuration.

use philo_config::{ConfigSource, ConfigValue, ProviderConfigLayer, ProviderConfigSnapshot};
use philo::provider::secret::SecretReference;
use philo::LlmError;

/// Builds the official bootstrap snapshot without resolving a secret value.
pub fn snapshot() -> Result<ProviderConfigSnapshot, LlmError> {
    let source = ConfigSource::environment_secret("consumer/env", "OPENAI_API_KEY")?;
    let layer = ProviderConfigLayer::new(source).with_credential(ConfigValue::set(
        SecretReference::environment_variable("OPENAI_API_KEY")?,
    ));
    Ok(ProviderConfigSnapshot::official_openai()?.merge_layers([layer])?)
}
