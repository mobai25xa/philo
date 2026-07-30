//! Stable consumer for stream, complete, and cancellation signatures.

use philo::{AssistantMessage, AssistantStream, GenerateRequest, LlmClient, LlmError, RequestControl};

/// Compiles the Stable streaming entry point and explicit request control.
pub async fn stream(
    client: &LlmClient,
    request: GenerateRequest,
    control: RequestControl,
) -> Result<AssistantStream, LlmError> {
    client.stream_with_control(request, control).await
}

/// Compiles the Stable completion entry point.
pub async fn complete(
    client: &LlmClient,
    request: GenerateRequest,
) -> Result<AssistantMessage, LlmError> {
    client.complete(request).await
}
