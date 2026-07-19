//! Frozen resource ceilings shared by request encoding and stream state machines.
#![allow(clippy::must_use_candidate)]

/// Fixed phase-two resource ceilings for Official `OpenAI` processing.
///
/// Production call sites use [`Self::official`]. Tests may construct lower
/// limits to exercise boundaries; increasing production ceilings requires a
/// contract change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceLimits {
    /// Maximum request body size before transport, including encoded images.
    pub max_request_body_bytes: usize,
    /// Maximum number of domain history messages accepted for one request.
    pub max_messages: usize,
    /// Maximum total UTF-8 text bytes across one history/context.
    pub max_total_text_bytes: usize,
    /// Maximum tool definitions declared on one request.
    pub max_tools: usize,
    /// Maximum description UTF-8 bytes for one tool definition.
    pub max_tool_description_bytes: usize,
    /// Maximum JSON schema UTF-8 bytes for one tool or response schema.
    pub max_schema_bytes: usize,
    /// Maximum nested object/array depth for local schema/argument checks.
    pub max_schema_depth: usize,
    /// Maximum tool calls observed in one generation.
    pub max_tool_calls: usize,
    /// Maximum raw argument bytes for one tool call accumulator.
    pub max_tool_arguments_bytes: usize,
    /// Maximum total raw argument bytes across all tool call accumulators.
    pub max_all_tool_arguments_bytes: usize,
    /// Maximum JSON array length accepted by local validators.
    pub max_json_array_items: usize,
    /// Maximum image parts on one request.
    pub max_images: usize,
    /// Maximum decoded inline image payload bytes.
    pub max_inline_image_bytes: usize,
    /// Maximum image URL UTF-8 bytes.
    pub max_image_url_bytes: usize,
}

impl ResourceLimits {
    /// Official `OpenAI` production ceilings frozen by the phase-two contract.
    pub const fn official() -> Self {
        Self {
            max_request_body_bytes: 64 * 1024 * 1024,
            max_messages: 1024,
            max_total_text_bytes: 16 * 1024 * 1024,
            max_tools: 128,
            max_tool_description_bytes: 1024,
            max_schema_bytes: 256 * 1024,
            max_schema_depth: 32,
            max_tool_calls: 64,
            max_tool_arguments_bytes: 1024 * 1024,
            max_all_tool_arguments_bytes: 4 * 1024 * 1024,
            max_json_array_items: 65_536,
            max_images: 128,
            max_inline_image_bytes: 20 * 1024 * 1024,
            max_image_url_bytes: 8192,
        }
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::official()
    }
}
