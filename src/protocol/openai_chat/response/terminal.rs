use std::collections::BTreeSet;

use super::super::wire::{ChatCompletionChunkWire, ChoiceWire};
use super::protocol;
use super::usage::parse_usage_details;
use crate::domain::{FinishReason, ResponseFormat, SchemaLimits, UsageDetails};
use crate::error::{
    LlmError, StructuredOutputError, StructuredOutputFailure, UnknownFinishReason,
    UnsupportedResponseSemantics,
};
use crate::provider::call_policy::ResponseLimits;

pub(super) struct StructuredTerminal {
    response_format: ResponseFormat,
    text_buffer: Option<String>,
    validated: bool,
}

impl StructuredTerminal {
    pub(super) fn new(response_format: ResponseFormat) -> Self {
        let text_buffer = if matches!(response_format, ResponseFormat::Text) {
            None
        } else {
            Some(String::new())
        };
        Self {
            response_format,
            text_buffer,
            validated: false,
        }
    }

    pub(super) fn push_text(&mut self, content: &str, max_bytes: usize) -> Result<(), LlmError> {
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
        if next > max_bytes {
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

    pub(super) fn validate(
        &mut self,
        finish_reason: &FinishReason,
        has_tools: bool,
        has_refusal: bool,
        limits: &ResponseLimits,
    ) -> Result<(), LlmError> {
        crate::domain::structured::validate_structured_response(
            &self.response_format,
            finish_reason,
            self.text_buffer.as_deref().unwrap_or_default(),
            has_tools,
            has_refusal,
            SchemaLimits {
                max_schema_bytes: usize::MAX,
                max_schema_depth: limits.max_schema_depth,
                max_json_array_items: limits.max_json_array_items,
            },
        )?;
        self.validated = true;
        Ok(())
    }

    pub(super) fn is_validated(&self) -> bool {
        self.validated
    }
}

pub(super) struct PreparedChunk {
    pub(super) finish_reason: Option<FinishReason>,
    pub(super) usage: Option<UsageDetails>,
}

impl PreparedChunk {
    pub(super) fn validate(
        chunk: &ChatCompletionChunkWire,
        finish_already_seen: bool,
    ) -> Result<Self, LlmError> {
        if chunk.choices.len() > 1 {
            return Err(UnsupportedResponseSemantics::new("multiple choices").into());
        }
        if chunk.choices.is_empty() && chunk.usage.is_none() {
            return Err(protocol("chunk has neither a choice nor usage"));
        }

        let finish_reason = if let Some(choice) = chunk.choices.first() {
            Self::validate_choice(choice, finish_already_seen)?
        } else {
            None
        };
        let usage = chunk
            .usage
            .as_ref()
            .map(parse_usage_details)
            .transpose()?
            .flatten();
        Ok(Self {
            finish_reason,
            usage,
        })
    }

    fn validate_choice(
        choice: &ChoiceWire,
        finish_already_seen: bool,
    ) -> Result<Option<FinishReason>, LlmError> {
        if choice.index != 0 {
            return Err(UnsupportedResponseSemantics::new("nonzero choice index").into());
        }
        if finish_already_seen {
            if choice.finish_reason.is_some() {
                return Err(protocol("duplicate finish reason"));
            }
            return Err(protocol("choice data received after finish reason"));
        }
        if let Some(delta) = &choice.delta {
            if delta.function_call.is_some() {
                return Err(UnsupportedResponseSemantics::new("function_call").into());
            }
            if delta
                .role
                .as_deref()
                .is_some_and(|role| role != "assistant")
            {
                return Err(UnsupportedResponseSemantics::new("delta.role").into());
            }
        }
        choice
            .finish_reason
            .as_deref()
            .map(parse_finish_reason)
            .transpose()
    }
}

fn parse_finish_reason(raw: &str) -> Result<FinishReason, LlmError> {
    match raw {
        "stop" => Ok(FinishReason::Stop),
        "length" => Ok(FinishReason::Length),
        "content_filter" => Ok(FinishReason::ContentFilter),
        "tool_calls" => Ok(FinishReason::ToolCalls),
        "function_call" => Err(UnsupportedResponseSemantics::new(raw).into()),
        _ => Err(UnknownFinishReason::new(bounded_label(raw, 64)).into()),
    }
}

pub(super) fn bounded_label(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

pub(super) fn record_field_names<'a>(
    destination: &mut BTreeSet<String>,
    scope: &str,
    names: impl Iterator<Item = &'a String>,
) {
    for name in names {
        let safe = if name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            name.as_str()
        } else {
            "<invalid-field-name>"
        };
        destination.insert(format!("{scope}.{safe}"));
    }
}
