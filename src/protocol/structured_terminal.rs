//! Shared bounded accumulator and validator for structured terminal responses.

use std::fmt;

use crate::domain::{FinishReason, ResponseFormat, SchemaLimits};
use crate::error::{LlmError, ProtocolError, StructuredOutputError, StructuredOutputFailure};

/// Protocol-private mechanism; concrete machines still own terminal recognition.
pub(super) struct StructuredTerminal {
    response_format: ResponseFormat,
    schema_limits: SchemaLimits,
    byte_limit: usize,
    text_buffer: Option<String>,
    validated: bool,
}

impl StructuredTerminal {
    pub(super) fn new(
        response_format: ResponseFormat,
        schema_limits: SchemaLimits,
        byte_limit: usize,
    ) -> Self {
        let text_buffer = if matches!(response_format, ResponseFormat::Text) {
            None
        } else {
            Some(String::new())
        };
        Self {
            response_format,
            schema_limits,
            byte_limit,
            text_buffer,
            validated: false,
        }
    }

    pub(super) fn push_answer_text(&mut self, content: &str) -> Result<(), LlmError> {
        let Some(buffer) = &mut self.text_buffer else {
            return Ok(());
        };
        let next = buffer.len().checked_add(content.len()).ok_or_else(|| {
            StructuredOutputError::new(
                "structured_output",
                StructuredOutputFailure::TooLarge,
                None,
                "structured output byte count overflowed",
            )
        })?;
        if next > self.byte_limit {
            return Err(StructuredOutputError::new(
                "structured_output",
                StructuredOutputFailure::TooLarge,
                None,
                "structured output exceeds the configured byte limit",
            )
            .into());
        }
        buffer.push_str(content);
        Ok(())
    }

    pub(super) fn validate_before_done(
        &mut self,
        finish_reason: &FinishReason,
        has_tools: bool,
        has_refusal: bool,
    ) -> Result<(), LlmError> {
        if self.validated {
            return Err(ProtocolError::new(
                "structured terminal validation was attempted more than once",
            )
            .into());
        }
        crate::domain::structured::validate_structured_response(
            &self.response_format,
            finish_reason,
            self.text_buffer.as_deref().unwrap_or_default(),
            has_tools,
            has_refusal,
            self.schema_limits,
        )?;
        self.validated = true;
        Ok(())
    }

    pub(super) const fn is_validated(&self) -> bool {
        self.validated
    }
}

impl fmt::Debug for StructuredTerminal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StructuredTerminal")
            .field("buffers_structured_text", &self.text_buffer.is_some())
            .field(
                "buffered_bytes",
                &self.text_buffer.as_ref().map_or(0, String::len),
            )
            .field("byte_limit", &self.byte_limit)
            .field("validated", &self.validated)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::StructuredTerminal;
    use crate::domain::{FinishReason, ResponseFormat, SchemaLimits};
    use crate::error::{LlmError, StructuredOutputFailure};

    #[test]
    fn text_mode_does_not_allocate_or_apply_the_structured_buffer_limit() {
        let mut terminal =
            StructuredTerminal::new(ResponseFormat::Text, SchemaLimits::official(), 1);
        terminal
            .push_answer_text("unbounded-by-this-helper")
            .unwrap();
        assert!(format!("{terminal:?}").contains("buffers_structured_text: false"));
    }

    #[test]
    fn structured_output_limit_fails_during_accumulation() {
        let mut terminal =
            StructuredTerminal::new(ResponseFormat::JsonObject, SchemaLimits::official(), 2);
        let error = terminal
            .push_answer_text("{}")
            .and_then(|()| terminal.push_answer_text("x"))
            .unwrap_err();
        assert!(matches!(
            error,
            LlmError::StructuredOutput(inner)
                if inner.reason() == StructuredOutputFailure::TooLarge
        ));
        assert!(!terminal.is_validated());
    }

    #[test]
    fn terminal_validation_succeeds_exactly_once() {
        let mut terminal =
            StructuredTerminal::new(ResponseFormat::JsonObject, SchemaLimits::official(), 16);
        terminal.push_answer_text("{}").unwrap();
        terminal
            .validate_before_done(&FinishReason::Stop, false, false)
            .unwrap();
        assert!(terminal.is_validated());
        assert!(
            terminal
                .validate_before_done(&FinishReason::Stop, false, false)
                .is_err()
        );
    }
}
