//! Lists built-in provider registrations without building a request or reading a Secret.

use std::error::Error;

use philo::ProviderRegistry;

fn main() -> Result<(), Box<dyn Error>> {
    let registry = ProviderRegistry::with_official_openai()?;
    for registration in registry.list()? {
        println!("{} {}", registration.provider_id(), registration.version());
    }
    Ok(())
}
