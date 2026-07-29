//! Public client facade and request lifecycle orchestration.

mod lifecycle;

pub use lifecycle::{AssistantStream, LlmClient, RequestControl};

#[cfg(test)]
mod http_e2e_tests;
#[cfg(test)]
mod release_tests;
