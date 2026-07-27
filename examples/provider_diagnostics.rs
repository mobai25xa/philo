//! Reads capability decisions and their evidence independently without sending a request.

use std::error::Error;

use philo::ModelId;
use philo_presets::OpenRouterProfile;

fn main() -> Result<(), Box<dyn Error>> {
    let runtime = OpenRouterProfile::from_api_key("resolved-by-the-application")?.build()?;
    let model = ModelId::new("nvidia/nemotron-3-ultra-550b-a55b:free")?;
    let entry = runtime.model_entry(&model).expect("preset catalog entry");
    println!("availability={:?}", entry.support_status);
    println!("evidence={}", entry.source.id());
    println!("reviewed_at={}", entry.source.reviewed_at());
    println!("stale={}", entry.source.is_stale_on("2026-07-24")?);
    Ok(())
}
