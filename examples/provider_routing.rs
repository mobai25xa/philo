//! Resolves typed `OpenRouter` routing without an arbitrary payload escape hatch.

use std::error::Error;

use philo::{
    OpenRouterRoutingContract, OpenRouterRoutingPatch, PolicySource, RoutingField, RoutingSort,
};

fn main() -> Result<(), Box<dyn Error>> {
    let contract = OpenRouterRoutingContract::new(OpenRouterRoutingPatch::from_source(
        PolicySource::ProviderProfile,
    ));
    let request =
        OpenRouterRoutingPatch::from_source(PolicySource::Request).with_sort(RoutingSort::Latency);
    let resolved = contract.resolve(Some(&request))?;

    println!("sort source={:?}", resolved.source(RoutingField::Sort));
    Ok(())
}
