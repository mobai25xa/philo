//! Compatibility and capability cross-validation.

use crate::domain::{CapabilityStatus, StreamUsagePolicy};
use crate::error::LlmError;
use crate::provider::ProviderCapabilities;

use super::{CompatProfile, ToolArgumentsCompat};

/// Rejects compatibility policies that contradict selected capabilities.
///
/// # Errors
///
/// Returns a configuration error when a typed compatibility policy requires a
/// capability that is not explicitly supported.
pub fn validate_compat(
    compat: &CompatProfile,
    capabilities: &ProviderCapabilities,
) -> Result<(), LlmError> {
    if matches!(
        compat.request().stream_usage,
        StreamUsagePolicy::IncludeUsage
    ) && !matches!(capabilities.streaming_usage, CapabilityStatus::Supported)
    {
        return Err(LlmError::Configuration(
            "stream usage compatibility requires explicit streaming usage support".to_owned(),
        ));
    }
    if matches!(
        compat.response().tool_arguments,
        ToolArgumentsCompat::StringOrObject
    ) && !matches!(capabilities.function_tools, CapabilityStatus::Supported)
    {
        return Err(LlmError::Configuration(
            "tool argument compatibility requires explicit function tool support".to_owned(),
        ));
    }
    Ok(())
}
