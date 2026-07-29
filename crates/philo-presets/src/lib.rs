//! Declarative third-party provider presets built on philo's public definition API.
//!
//! # Stability
//!
//! This crate is **Experimental**. Provider products, model catalogs, and
//! compatibility behavior can drift independently from the core SDK. Every
//! preset requires a named owner, Canary evidence, and an expiry review before
//! publication.

mod common;
mod deepseek;
mod openrouter;
mod zai;

pub use deepseek::DeepSeekProfile;
pub use openrouter::{OpenRouterAttribution, OpenRouterProfile};
pub use zai::{ZaiCodingProfile, ZaiStandardProfile};
