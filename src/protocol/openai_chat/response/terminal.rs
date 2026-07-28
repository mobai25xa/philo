use std::collections::BTreeSet;

use super::super::wire::{ChatCompletionChunkWire, ChoiceWire};
use super::protocol;
use super::usage::parse_usage_details;
use crate::domain::{FinishReason, UsageDetails};
use crate::error::{LlmError, UnknownFinishReason, UnsupportedResponseSemantics};
use crate::provider::{FinishReasonCompat, UsageCompat};

const MAX_RECORDED_UNKNOWN_FIELDS: usize = 256;
const TRUNCATED_UNKNOWN_FIELDS: &str = "diagnostic.<truncated>";

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
        if destination.len() >= MAX_RECORDED_UNKNOWN_FIELDS.saturating_sub(1) {
            destination.insert(TRUNCATED_UNKNOWN_FIELDS.to_owned());
            break;
        }
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
