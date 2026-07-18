//! `philo` is a secure, streaming-first Rust SDK for LLM applications.
//!
//! Phase one implements the frozen `philo/openai-chat-p1` contract. The crate
//! currently exposes only stable project metadata while the domain and protocol
//! modules are implemented behind reviewed task boundaries.
//!
//! # Stability
//!
//! The public API is experimental during the `0.x` series. The phase-one behavior
//! contract is frozen, but Rust type layouts may change with release notes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// The public SDK, Cargo package, and library crate name.
pub const SDK_NAME: &str = "philo";

/// The version of this crate build.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The identifier of the frozen phase-one behavior contract.
pub const PHASE_ONE_CONTRACT_ID: &str = "philo/openai-chat-p1";

/// The version of the frozen phase-one behavior contract.
pub const PHASE_ONE_CONTRACT_VERSION: &str = "1.0.0";

#[cfg(test)]
mod tests {
    use super::{PHASE_ONE_CONTRACT_ID, PHASE_ONE_CONTRACT_VERSION, SDK_NAME, SDK_VERSION};

    #[test]
    fn published_metadata_matches_frozen_decisions() {
        assert_eq!(SDK_NAME, "philo");
        assert_eq!(SDK_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(PHASE_ONE_CONTRACT_ID, "philo/openai-chat-p1");
        assert_eq!(PHASE_ONE_CONTRACT_VERSION, "1.0.0");
    }
}
