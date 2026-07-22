//! Frozen resource ceilings shared by request encoding and stream state machines.
#![allow(clippy::must_use_candidate)]

use crate::error::{ValidationError, ValidationReason};

/// Fixed phase-two resource ceilings for Official `OpenAI` processing.
///
/// Official profile construction and public convenience validation may use
/// [`Self::official`]. Planner, Driver, Executor, and `ResponseSession` consume
/// one resolved snapshot instead of reading defaults. Tests may construct lower
/// limits to exercise boundaries; increasing production ceilings requires a
/// contract change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
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
    /// Maximum buffered structured-output UTF-8 bytes before terminal validation.
    pub max_structured_output_bytes: usize,
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
            max_structured_output_bytes: 16 * 1024 * 1024,
        }
    }

    /// Creates a builder initialized with the official production ceilings.
    pub const fn builder() -> ResourceLimitsBuilder {
        ResourceLimitsBuilder {
            limits: Self::official(),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        let fields = [
            ("max_request_body_bytes", self.max_request_body_bytes),
            ("max_messages", self.max_messages),
            ("max_total_text_bytes", self.max_total_text_bytes),
            ("max_tools", self.max_tools),
            (
                "max_tool_description_bytes",
                self.max_tool_description_bytes,
            ),
            ("max_schema_bytes", self.max_schema_bytes),
            ("max_schema_depth", self.max_schema_depth),
            ("max_tool_calls", self.max_tool_calls),
            ("max_tool_arguments_bytes", self.max_tool_arguments_bytes),
            (
                "max_all_tool_arguments_bytes",
                self.max_all_tool_arguments_bytes,
            ),
            ("max_json_array_items", self.max_json_array_items),
            ("max_images", self.max_images),
            ("max_inline_image_bytes", self.max_inline_image_bytes),
            ("max_image_url_bytes", self.max_image_url_bytes),
            (
                "max_structured_output_bytes",
                self.max_structured_output_bytes,
            ),
        ];
        if let Some((field, _)) = fields.into_iter().find(|(_, value)| *value == 0) {
            return Err(ValidationError::new(
                field,
                ValidationReason::Zero,
                "resource limit must be positive",
            ));
        }
        if self.max_all_tool_arguments_bytes < self.max_tool_arguments_bytes {
            return Err(ValidationError::new(
                "max_all_tool_arguments_bytes",
                ValidationReason::OutOfRange,
                "aggregate tool argument limit must cover one tool call",
            ));
        }
        Ok(())
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self::official()
    }
}

/// Builder for a complete, validated [`ResourceLimits`] value.
///
/// The builder starts from [`ResourceLimits::official`], so callers only need
/// to override limits that differ from the official profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct ResourceLimitsBuilder {
    limits: ResourceLimits,
}

impl ResourceLimitsBuilder {
    /// Sets the maximum encoded request body size.
    pub const fn with_max_request_body_bytes(mut self, value: usize) -> Self {
        self.limits.max_request_body_bytes = value;
        self
    }

    /// Sets the maximum history message count.
    pub const fn with_max_messages(mut self, value: usize) -> Self {
        self.limits.max_messages = value;
        self
    }

    /// Sets the maximum total request text bytes.
    pub const fn with_max_total_text_bytes(mut self, value: usize) -> Self {
        self.limits.max_total_text_bytes = value;
        self
    }

    /// Sets the maximum declared tool count.
    pub const fn with_max_tools(mut self, value: usize) -> Self {
        self.limits.max_tools = value;
        self
    }

    /// Sets the maximum bytes in one tool description.
    pub const fn with_max_tool_description_bytes(mut self, value: usize) -> Self {
        self.limits.max_tool_description_bytes = value;
        self
    }

    /// Sets the maximum bytes in one encoded schema.
    pub const fn with_max_schema_bytes(mut self, value: usize) -> Self {
        self.limits.max_schema_bytes = value;
        self
    }

    /// Sets the maximum schema nesting depth.
    pub const fn with_max_schema_depth(mut self, value: usize) -> Self {
        self.limits.max_schema_depth = value;
        self
    }

    /// Sets the maximum tool calls in one response.
    pub const fn with_max_tool_calls(mut self, value: usize) -> Self {
        self.limits.max_tool_calls = value;
        self
    }

    /// Sets the maximum argument bytes for one tool call.
    pub const fn with_max_tool_arguments_bytes(mut self, value: usize) -> Self {
        self.limits.max_tool_arguments_bytes = value;
        self
    }

    /// Sets the maximum aggregate argument bytes across all tool calls.
    pub const fn with_max_all_tool_arguments_bytes(mut self, value: usize) -> Self {
        self.limits.max_all_tool_arguments_bytes = value;
        self
    }

    /// Sets the maximum array length accepted by local JSON validators.
    pub const fn with_max_json_array_items(mut self, value: usize) -> Self {
        self.limits.max_json_array_items = value;
        self
    }

    /// Sets the maximum image count in one request.
    pub const fn with_max_images(mut self, value: usize) -> Self {
        self.limits.max_images = value;
        self
    }

    /// Sets the maximum decoded bytes in one inline image.
    pub const fn with_max_inline_image_bytes(mut self, value: usize) -> Self {
        self.limits.max_inline_image_bytes = value;
        self
    }

    /// Sets the maximum UTF-8 bytes in one image URL.
    pub const fn with_max_image_url_bytes(mut self, value: usize) -> Self {
        self.limits.max_image_url_bytes = value;
        self
    }

    /// Sets the maximum buffered structured-output bytes.
    pub const fn with_max_structured_output_bytes(mut self, value: usize) -> Self {
        self.limits.max_structured_output_bytes = value;
        self
    }

    /// Validates and returns the complete resource limits.
    pub fn build(self) -> Result<ResourceLimits, ValidationError> {
        self.limits.validate()?;
        Ok(self.limits)
    }
}

impl Default for ResourceLimitsBuilder {
    fn default() -> Self {
        ResourceLimits::builder()
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceLimits;
    use crate::error::ValidationReason;

    #[test]
    fn builder_starts_from_official_limits_and_supports_targeted_overrides() {
        assert_eq!(
            ResourceLimits::builder().build().unwrap(),
            ResourceLimits::official()
        );

        let limits = ResourceLimits::builder()
            .with_max_messages(7)
            .with_max_structured_output_bytes(4096)
            .build()
            .unwrap();
        assert_eq!(limits.max_messages, 7);
        assert_eq!(limits.max_structured_output_bytes, 4096);
        assert_eq!(
            limits.max_request_body_bytes,
            ResourceLimits::official().max_request_body_bytes
        );
    }

    #[test]
    fn builder_rejects_zero_and_inconsistent_tool_argument_limits() {
        let zero = ResourceLimits::builder()
            .with_max_messages(0)
            .build()
            .unwrap_err();
        assert_eq!(zero.field(), "max_messages");
        assert_eq!(zero.reason(), ValidationReason::Zero);

        let inconsistent = ResourceLimits::builder()
            .with_max_tool_arguments_bytes(10)
            .with_max_all_tool_arguments_bytes(9)
            .build()
            .unwrap_err();
        assert_eq!(inconsistent.field(), "max_all_tool_arguments_bytes");
        assert_eq!(inconsistent.reason(), ValidationReason::OutOfRange);
    }
}
