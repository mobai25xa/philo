//! Builds every Experimental third-party Provider preset without making a request.

use std::error::Error;

use philo_presets::{
    DeepSeekProfile, OpenRouterAttribution, OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile,
};

fn main() -> Result<(), Box<dyn Error>> {
    let runtimes = [
        OpenRouterProfile::from_api_key("replace-through-secret-resolution")?
            .with_attribution(OpenRouterAttribution::new(
                "https://app.example",
                "example application",
            )?)
            .build()?,
        DeepSeekProfile::from_api_key("replace-through-secret-resolution")?.build()?,
        ZaiStandardProfile::from_api_key("replace-through-secret-resolution")?
            .with_accept_language("en-US")?
            .build()?,
        ZaiCodingProfile::from_api_key("replace-through-secret-resolution")?
            .with_accept_language("en-US")?
            .build()?,
    ];

    for runtime in runtimes {
        println!(
            "{}/{} -> {:?}",
            runtime.provider_id(),
            runtime.product_id(),
            runtime.endpoint()
        );
    }
    Ok(())
}
