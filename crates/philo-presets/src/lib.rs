//! Declarative third-party provider presets built on philo's public definition API.

mod common;
mod deepseek;
mod openrouter;
mod zai;

pub use deepseek::DeepSeekProfile;
pub use openrouter::{OpenRouterAttribution, OpenRouterProfile};
pub use zai::{ZaiCodingProfile, ZaiStandardProfile};
