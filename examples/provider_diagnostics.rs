//! Produces a value-free diagnostic snapshot without sending the request.

use std::error::Error;

use philo::{GenerateRequest, Message, ModelRef, OpenRouterProfile, ProviderRequestOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = OpenRouterProfile::from_api_key("resolved-by-the-application")?.build()?;
    let request = GenerateRequest::new(
        ModelRef::new("openrouter", "nvidia/nemotron-3-ultra-550b-a55b:free")?,
        vec![Message::user("This text is never retained by diagnostics.")],
    );
    let diagnostics =
        runtime.diagnostics_for_request(&request, &ProviderRequestOptions::new(), "2026-07-24")?;

    println!("{diagnostics}");
    for header in diagnostics.headers() {
        println!(
            "header={} owner={:?} protected={} sensitive={}",
            header.name(),
            header.source(),
            header.is_protected(),
            header.is_sensitive()
        );
    }
    Ok(())
}
