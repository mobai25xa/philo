//! Layered `OpenAI`-compatibility declaration and merge for the philo SDK core.
//!
//! # Why this is not in the core
//!
//! There are two different questions inside what used to be `provider/compat`:
//!
//! 1. **What is the contract?** Which finish reasons are legal, where usage
//!    counters live, whether an inline error terminates the stream, which field
//!    carries the output-token ceiling. Getting these wrong produces a wrong
//!    result or inaccurate billing, so by the FR-000 criterion they belong to
//!    the core — and they now live in
//!    [`philo::provider::protocol_contract`], fixed when a
//!    [`ProviderDefinition`](philo::provider::ProviderDefinition) is built.
//!
//! 2. **Which declaration wins?** A sparse patch per layer, a precedence order
//!    between protocol default / provider profile / model profile, per-leaf
//!    overlay and provenance. Getting *this* wrong means the deployment is
//!    configured differently than intended — inconvenience, not illegality.
//!    That is this crate.
//!
//! # Shape
//!
//! ```text
//! [CompatPatch, CompatPatch, ...]   ordered, sparse, each with a source
//!     -> resolve_compat
//!     -> philo::provider::CompatProfile   (resolved, provenance-carrying)
//!     -> ProviderDefinitionBuilder::with_openai_chat_compat
//! ```
//!
//! The core never merges. It receives one already-resolved contract per model
//! and carries it unchanged into every request.

mod merge;

pub use merge::{CompatPatch, resolve_compat};
