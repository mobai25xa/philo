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
use crate::provider::{FinishReasonCompat, UsageCompat};

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
    pub(super) duplicate_finish: bool,
    pub(super) usage: Option<UsageDetails>,
}

impl PreparedChunk {
    pub(super) fn validate(
        chunk: &ChatCompletionChunkWire,
        observed_finish: Option<&FinishReason>,
        duplicate_finish_seen: bool,
        finish_compat: FinishReasonCompat,
        usage_compat: UsageCompat,
    ) -> Result<Self, LlmError> {
        if chunk.choices.len() > 1 {
            return Err(UnsupportedResponseSemantics::new("multiple choices").into());
        }
        if chunk.choices.is_empty() && chunk.usage.is_none() {
            return Err(protocol("chunk has neither a choice nor usage"));
        }

        let (finish_reason, duplicate_finish) = if let Some(choice) = chunk.choices.first() {
            Self::validate_choice(
                choice,
                observed_finish,
                duplicate_finish_seen,
                finish_compat,
            )?
        } else {
            (None, false)
        };
        let usage = chunk
            .usage
            .as_ref()
            .map(|usage| {
                parse_usage_details(
                    usage,
                    matches!(usage_compat, UsageCompat::OpenAiDropInconsistentReasoning),
                )
            })
            .transpose()?
            .flatten();
        Ok(Self {
            finish_reason,
            duplicate_finish,
            usage,
        })
    }

    fn validate_choice(
        choice: &ChoiceWire,
        observed_finish: Option<&FinishReason>,
        duplicate_finish_seen: bool,
        finish_compat: FinishReasonCompat,
    ) -> Result<(Option<FinishReason>, bool), LlmError> {
        if choice.index != 0 {
            return Err(UnsupportedResponseSemantics::new("nonzero choice index").into());
        }
        if let Some(observed_finish) = observed_finish {
            let Some(raw_finish) = choice.finish_reason.as_deref() else {
                return Err(protocol("choice data received after finish reason"));
            };
            if matches!(finish_compat, FinishReasonCompat::StrictOpenAi) {
                return Err(protocol("duplicate finish reason"));
            }
            if duplicate_finish_seen {
                return Err(protocol("multiple duplicate finish reasons"));
            }
            let repeated_finish = parse_finish_reason(raw_finish)?;
            if &repeated_finish != observed_finish {
                return Err(protocol("conflicting duplicate finish reason"));
            }
            if !choice.delta.as_ref().is_none_or(delta_is_empty) {
                return Err(protocol("choice data received after finish reason"));
            }
            return Ok((None, true));
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
        let finish_reason = choice
            .finish_reason
            .as_deref()
            .map(parse_finish_reason)
            .transpose()?;
        Ok((finish_reason, false))
    }
}

fn delta_is_empty(delta: &super::super::wire::DeltaWire) -> bool {
    delta.role.as_deref().is_none_or(|role| role == "assistant")
        && delta.content.as_deref().is_none_or(str::is_empty)
        && delta.refusal.as_deref().is_none_or(str::is_empty)
        && delta.tool_calls.as_ref().is_none_or(Vec::is_empty)
        && delta.function_call.is_none()
        && delta.extra.iter().all(|(name, value)| {
            matches!(
                name.as_str(),
                "reasoning" | "reasoning_content" | "reasoning_details"
            ) && empty_extension_value(value)
        })
}

fn empty_extension_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(value) => value.is_empty(),
        serde_json::Value::Array(value) => value.is_empty(),
        serde_json::Value::Object(value) => value.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
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
