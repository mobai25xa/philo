//! Resolves a typed deployment/model endpoint without string interpolation.

use std::error::Error;

use philo::provider::endpoint::resolve_official_for;
use philo::{
    DeploymentId, EndpointConfig, EndpointQuery, EndpointTemplate, EndpointValues, ProductId,
    ProviderModelId,
};

fn main() -> Result<(), Box<dyn Error>> {
    let config = EndpointConfig::base_and_template(
        "https://api.example.com/proxy",
        EndpointTemplate::parse(
            "deployments/{deployment}/models/{provider_model}/chat/completions",
        )?,
        EndpointQuery::new(),
    )?;
    let product = ProductId::new("deployment-chat")?;
    let provider_model = ProviderModelId::new("provider-model")?;
    let deployment = DeploymentId::new("deployment-a")?;
    let endpoint = resolve_official_for(
        &config,
        EndpointValues::new(&product, &provider_model, Some(&deployment)),
    )?;

    println!("{endpoint:?}");
    Ok(())
}
