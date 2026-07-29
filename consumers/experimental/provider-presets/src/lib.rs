//! Experimental consumer for third-party provider presets.

use philo_presets::{DeepSeekProfile, OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile};

/// Keeps the Experimental preset constructors in an isolated compile contract.
pub fn public_types_are_reachable() {
    let _ = std::mem::size_of::<DeepSeekProfile>();
    let _ = std::mem::size_of::<OpenRouterProfile>();
    let _ = std::mem::size_of::<ZaiCodingProfile>();
    let _ = std::mem::size_of::<ZaiStandardProfile>();
}
