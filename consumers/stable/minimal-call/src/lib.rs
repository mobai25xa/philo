//! Stable consumer for the smallest provider-neutral request surface.

use philo::{GenerateRequest, LlmError, Message, ModelRef};

/// Builds a request using only crate-root Stable exports.
pub fn request() -> Result<GenerateRequest, LlmError> {
    Ok(GenerateRequest::new(
        ModelRef::new("official-openai", "consumer-model")?,
        vec![Message::user("hello")],
    ))
}
