//! Public client facade and request lifecycle orchestration.

mod lifecycle;

pub use lifecycle::{AssistantStream, LlmClient, RequestControl};
