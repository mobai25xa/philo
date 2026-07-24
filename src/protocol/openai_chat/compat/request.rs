//! Private request compatibility hooks.

use crate::domain::ToolResultNamePolicy;
use crate::provider::MaxOutputTokensWireFormat;

pub(in crate::protocol::openai_chat) fn output_token_fields(
    value: Option<u32>,
    format: MaxOutputTokensWireFormat,
) -> (Option<u32>, Option<u32>) {
    match format {
        MaxOutputTokensWireFormat::MaxCompletionTokens => (value, None),
        MaxOutputTokensWireFormat::MaxTokens => (None, value),
    }
}

pub(in crate::protocol::openai_chat) const fn tool_result_name(
    name: &str,
    policy: ToolResultNamePolicy,
) -> Option<&str> {
    match policy {
        ToolResultNamePolicy::Omit => None,
        ToolResultNamePolicy::Include | ToolResultNamePolicy::Require => Some(name),
    }
}
