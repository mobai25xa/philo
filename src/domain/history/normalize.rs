//! History normalization, tool pairing, and official replay boundaries.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref
)]

use std::collections::{BTreeMap, BTreeSet};

use super::super::request::CapabilityStatus;
use super::super::{
    ContentPart, Message, MessageRole, ToolCall, ToolCallId, ToolName, ToolResultMessage,
};
use crate::error::{HistoryError, HistoryFailure};

use super::diagnostics::{DiagnosticCode, DiagnosticCounter, IdMapping, NormalizedContext};
use super::policy::{
    DialectPolicy, HistoryCapabilities, HistoryPolicy, ThinkingReplayPolicy, ToolCallIdPolicy,
    UnsupportedContentPolicy,
};

/// Normalizes domain history for a target profile without mutating the input.
pub fn normalize_history(
    messages: &[Message],
    capabilities: &HistoryCapabilities,
    dialect: &DialectPolicy,
    policy: &HistoryPolicy,
) -> Result<NormalizedContext, HistoryError> {
    let limits = crate::domain::ResourceLimits::official();
    normalize_history_with_limits(
        messages,
        capabilities,
        dialect,
        policy,
        limits.max_messages,
        limits.max_total_text_bytes,
    )
}

/// Normalizes history with a resolved fail-closed resource ceiling.
pub(crate) fn normalize_history_with_limits(
    messages: &[Message],
    capabilities: &HistoryCapabilities,
    dialect: &DialectPolicy,
    policy: &HistoryPolicy,
    max_messages: usize,
    max_total_text_bytes: usize,
) -> Result<NormalizedContext, HistoryError> {
    if messages.len() > max_messages {
        return Err(HistoryError::new(
            "messages",
            HistoryFailure::TooManyMessages,
            None,
            "history exceeds the allowed message count",
        ));
    }

    let mut output = Vec::with_capacity(messages.len());
    let mut diagnostics = DiagnosticCounter::default();
    let mut id_mappings = Vec::new();
    let mut total_text_bytes = 0usize;
    let mut index = 0usize;

    while index < messages.len() {
        let message = &messages[index];
        match message.role() {
            MessageRole::Tool => {
                return Err(HistoryError::new(
                    format!("messages[{index}]"),
                    HistoryFailure::ResultBeforeCall,
                    Some(format!("messages[{index}]")),
                    "tool result appeared before an assistant tool-call turn",
                ));
            }
            MessageRole::Developer => {
                if capabilities.developer_role != CapabilityStatus::Supported {
                    return Err(HistoryError::new(
                        format!("messages[{index}].role"),
                        HistoryFailure::UnsupportedContent,
                        Some(format!("messages[{index}]")),
                        "developer role is not supported by the target profile",
                    ));
                }
                let normalized = normalize_plain_message(
                    message,
                    index,
                    capabilities,
                    policy,
                    &mut diagnostics,
                    &mut total_text_bytes,
                    max_total_text_bytes,
                )?;
                if let Some(message) = normalized {
                    output.push(message);
                }
                index += 1;
            }
            MessageRole::System | MessageRole::User => {
                let normalized = normalize_plain_message(
                    message,
                    index,
                    capabilities,
                    policy,
                    &mut diagnostics,
                    &mut total_text_bytes,
                    max_total_text_bytes,
                )?;
                if let Some(message) = normalized {
                    output.push(message);
                }
                index += 1;
            }
            MessageRole::Assistant => {
                let assistant_index = index;
                let content = message.content();
                let tool_calls = collect_assistant_tool_calls(content, assistant_index)?;
                if content.is_empty() {
                    diagnostics.increment(DiagnosticCode::RemovedEmptyAssistant);
                    index += 1;
                    continue;
                }

                let (assistant_message, local_mappings) = normalize_assistant_message(
                    content,
                    assistant_index,
                    dialect.tool_call_id,
                    policy,
                    capabilities,
                    &mut diagnostics,
                    &mut BTreeSet::new(),
                    &mut total_text_bytes,
                    max_total_text_bytes,
                )?;
                let mapping_start = id_mappings.len();
                id_mappings.extend(local_mappings);

                if tool_calls.is_empty() {
                    if let Some(message) = assistant_message {
                        output.push(message);
                    } else {
                        diagnostics.increment(DiagnosticCode::RemovedEmptyAssistant);
                    }
                    index += 1;
                    continue;
                }

                let Some(assistant_message) = assistant_message else {
                    return Err(HistoryError::new(
                        format!("messages[{assistant_index}]"),
                        HistoryFailure::InvalidMessageOrder,
                        Some(format!("messages[{assistant_index}]")),
                        "assistant tool-call turn became empty after normalization",
                    ));
                };

                let turn_mappings = &id_mappings[mapping_start..];
                let mut expected = BTreeMap::new();
                for (original_id, name) in &tool_calls {
                    let normalized_id = turn_mappings
                        .iter()
                        .find(|mapping| mapping.original().as_str() == original_id.as_str())
                        .map_or_else(
                            || original_id.as_str(),
                            |mapping| mapping.normalized().as_str(),
                        )
                        .to_owned();
                    if expected.insert(normalized_id, name.clone()).is_some() {
                        return Err(HistoryError::new(
                            format!("messages[{assistant_index}]"),
                            HistoryFailure::DuplicateToolCall,
                            Some(format!("messages[{assistant_index}]")),
                            "assistant turn contains duplicate tool call ids",
                        ));
                    }
                }

                output.push(assistant_message);
                let mut pending = expected.clone();
                index += 1;
                let mut results: Vec<ToolResultMessage> = Vec::new();
                while index < messages.len() && messages[index].role() == MessageRole::Tool {
                    let result = messages[index].tool_result().ok_or_else(|| {
                        HistoryError::new(
                            format!("messages[{index}]"),
                            HistoryFailure::InvalidMessageOrder,
                            Some(format!("messages[{index}]")),
                            "tool role message is missing a tool result payload",
                        )
                    })?;
                    let original_id = result.tool_call_id().as_str();
                    let mapped_id = resolve_mapped_id(original_id, turn_mappings, &tool_calls)?;
                    match pending.remove(mapped_id.as_str()) {
                        Some(_) => {}
                        None if expected.contains_key(mapped_id.as_str())
                            || results.iter().any(|existing: &ToolResultMessage| {
                                existing.tool_call_id().as_str() == mapped_id.as_str()
                            }) =>
                        {
                            return Err(HistoryError::new(
                                format!("messages[{index}].tool_call_id"),
                                HistoryFailure::DuplicateToolResult,
                                Some(format!("messages[{index}]")),
                                "tool result id was submitted more than once",
                            ));
                        }
                        None => {
                            return Err(HistoryError::new(
                                format!("messages[{index}].tool_call_id"),
                                HistoryFailure::UnknownToolCall,
                                Some(format!("messages[{index}]")),
                                "tool result references an unknown tool call id",
                            ));
                        }
                    }
                    let remapped = remap_tool_result(
                        result,
                        &mapped_id,
                        &mut total_text_bytes,
                        max_total_text_bytes,
                    )?;
                    results.push(remapped);
                    index += 1;
                }

                if !pending.is_empty() {
                    return Err(HistoryError::new(
                        format!("messages[{assistant_index}]"),
                        HistoryFailure::MissingToolResult,
                        Some(format!("messages[{assistant_index}]")),
                        "assistant tool-call turn is missing one or more tool results",
                    ));
                }

                for result in results {
                    output.push(Message::from_tool_result(result));
                }
            }
        }
    }

    if output.len() > max_messages {
        return Err(HistoryError::new(
            "messages",
            HistoryFailure::TooManyMessages,
            None,
            "history exceeds the allowed message count",
        ));
    }

    Ok(NormalizedContext::from_parts(
        output,
        id_mappings,
        diagnostics.into_vec(),
    ))
}

fn normalize_plain_message(
    message: &Message,
    index: usize,
    capabilities: &HistoryCapabilities,
    policy: &HistoryPolicy,
    diagnostics: &mut DiagnosticCounter,
    total_text_bytes: &mut usize,
    max_total_text_bytes: usize,
) -> Result<Option<Message>, HistoryError> {
    if message.tool_result().is_some() {
        return Err(HistoryError::new(
            format!("messages[{index}]"),
            HistoryFailure::InvalidMessageOrder,
            Some(format!("messages[{index}]")),
            "non-tool role must not carry a tool result payload",
        ));
    }
    let mut parts = Vec::with_capacity(message.content().len());
    for (part_index, part) in message.content().iter().enumerate() {
        if let Some(normalized) = normalize_content_part(
            part,
            index,
            part_index,
            capabilities,
            policy,
            diagnostics,
            total_text_bytes,
            max_total_text_bytes,
            false,
        )? {
            parts.push(normalized);
        }
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Message::new(message.role(), parts)))
    }
}

fn normalize_assistant_message(
    content: &[ContentPart],
    index: usize,
    id_policy: ToolCallIdPolicy,
    policy: &HistoryPolicy,
    capabilities: &HistoryCapabilities,
    diagnostics: &mut DiagnosticCounter,
    occupied_ids: &mut BTreeSet<String>,
    total_text_bytes: &mut usize,
    max_total_text_bytes: usize,
) -> Result<(Option<Message>, Vec<IdMapping>), HistoryError> {
    let mut parts = Vec::with_capacity(content.len());
    let mut mappings = Vec::new();
    let mut seen_tool_call = false;

    for (part_index, part) in content.iter().enumerate() {
        match part {
            ContentPart::ToolCall(call) => {
                seen_tool_call = true;
                let (normalized_call, mapping) =
                    normalize_tool_call(call, index, part_index, id_policy, occupied_ids)?;
                if let Some(mapping) = mapping {
                    diagnostics.increment(DiagnosticCode::SanitizedToolCallId);
                    mappings.push(mapping);
                }
                parts.push(ContentPart::ToolCall(normalized_call));
            }
            ContentPart::Text { .. }
            | ContentPart::Image(_)
            | ContentPart::Thinking(_)
            | ContentPart::Refusal(_) => {
                if seen_tool_call {
                    return Err(HistoryError::new(
                        format!("messages[{index}].content[{part_index}]"),
                        HistoryFailure::InvalidMessageOrder,
                        Some(format!("messages[{index}].content[{part_index}]")),
                        "assistant content after tool calls is not allowed",
                    ));
                }
                if let Some(normalized) = normalize_content_part(
                    part,
                    index,
                    part_index,
                    capabilities,
                    policy,
                    diagnostics,
                    total_text_bytes,
                    max_total_text_bytes,
                    true,
                )? {
                    parts.push(normalized);
                }
            }
        }
    }

    if parts.is_empty() {
        Ok((None, mappings))
    } else {
        Ok((Some(Message::new(MessageRole::Assistant, parts)), mappings))
    }
}

fn normalize_content_part(
    part: &ContentPart,
    message_index: usize,
    part_index: usize,
    capabilities: &HistoryCapabilities,
    policy: &HistoryPolicy,
    diagnostics: &mut DiagnosticCounter,
    total_text_bytes: &mut usize,
    max_total_text_bytes: usize,
    allow_refusal: bool,
) -> Result<Option<ContentPart>, HistoryError> {
    let path = format!("messages[{message_index}].content[{part_index}]");
    match part {
        ContentPart::Text { text } => {
            add_text_bytes(total_text_bytes, text.len(), max_total_text_bytes)?;
            Ok(Some(ContentPart::text(text.clone())))
        }
        ContentPart::Image(_) => match capabilities.vision_input {
            CapabilityStatus::Supported => Ok(Some(part.clone())),
            CapabilityStatus::Unsupported | CapabilityStatus::Unknown => {
                if matches!(policy.unsupported_content, UnsupportedContentPolicy::Reject) {
                    Err(HistoryError::new(
                        path.clone(),
                        HistoryFailure::UnsupportedContent,
                        Some(path),
                        "image content is not supported by the target profile",
                    ))
                } else {
                    diagnostics.increment(DiagnosticCode::DroppedUnsupportedImage);
                    Ok(None)
                }
            }
        },
        ContentPart::Thinking(thinking) => match policy.thinking_replay {
            ThinkingReplayPolicy::DropAll => {
                if thinking.opaque().is_some() {
                    diagnostics.increment(DiagnosticCode::DroppedThinkingOpaque);
                }
                Ok(None)
            }
            ThinkingReplayPolicy::SameSourceOnly => {
                add_text_bytes(
                    total_text_bytes,
                    thinking.text().len(),
                    max_total_text_bytes,
                )?;
                Ok(Some(ContentPart::Thinking(thinking.clone())))
            }
        },
        ContentPart::Refusal(refusal) => {
            if !allow_refusal {
                return Err(HistoryError::new(
                    path.clone(),
                    HistoryFailure::UnsupportedContent,
                    Some(path),
                    "refusal content is only valid on assistant turns",
                ));
            }
            add_text_bytes(total_text_bytes, refusal.text().len(), max_total_text_bytes)?;
            Ok(Some(ContentPart::Refusal(refusal.clone())))
        }
        ContentPart::ToolCall(_) => Err(HistoryError::new(
            path.clone(),
            HistoryFailure::InvalidMessageOrder,
            Some(path),
            "tool calls are only valid on assistant turns",
        )),
    }
}

fn collect_assistant_tool_calls(
    content: &[ContentPart],
    index: usize,
) -> Result<Vec<(ToolCallId, ToolName)>, HistoryError> {
    let mut calls = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for (part_index, part) in content.iter().enumerate() {
        if let ContentPart::ToolCall(call) = part {
            if !seen_ids.insert(call.id().as_str().to_owned()) {
                return Err(HistoryError::new(
                    format!("messages[{index}].content[{part_index}].id"),
                    HistoryFailure::DuplicateToolCall,
                    Some(format!("messages[{index}].content[{part_index}]")),
                    "assistant turn contains duplicate tool call ids",
                ));
            }
            calls.push((call.id().clone(), call.name().clone()));
        }
    }
    Ok(calls)
}

fn normalize_tool_call(
    call: &ToolCall,
    message_index: usize,
    part_index: usize,
    policy: ToolCallIdPolicy,
    occupied_ids: &mut BTreeSet<String>,
) -> Result<(ToolCall, Option<IdMapping>), HistoryError> {
    match policy {
        ToolCallIdPolicy::Preserve => {
            if !occupied_ids.insert(call.id().as_str().to_owned()) {
                return Err(HistoryError::new(
                    format!("messages[{message_index}].content[{part_index}].id"),
                    HistoryFailure::DuplicateToolCall,
                    Some(format!("messages[{message_index}].content[{part_index}]")),
                    "tool call id collides with an earlier normalized id",
                ));
            }
            Ok((call.clone(), None))
        }
        ToolCallIdPolicy::OpenAi => {
            let sanitized = sanitize_openai_tool_call_id(call.id(), occupied_ids).map_err(
                |reason| match reason {
                    SanitizeError::Collision => HistoryError::new(
                        format!("messages[{message_index}].content[{part_index}].id"),
                        HistoryFailure::ToolCallIdCollision,
                        Some(format!("messages[{message_index}].content[{part_index}]")),
                        "sanitized tool call id collided with another call id",
                    ),
                    SanitizeError::Empty => HistoryError::new(
                        format!("messages[{message_index}].content[{part_index}].id"),
                        HistoryFailure::InvalidMessageOrder,
                        Some(format!("messages[{message_index}].content[{part_index}]")),
                        "tool call id became empty after sanitization",
                    ),
                },
            )?;
            let normalized_id = ToolCallId::new(sanitized).map_err(|_| {
                HistoryError::new(
                    format!("messages[{message_index}].content[{part_index}].id"),
                    HistoryFailure::InvalidMessageOrder,
                    Some(format!("messages[{message_index}].content[{part_index}]")),
                    "sanitized tool call id is invalid",
                )
            })?;
            let mapping = if normalized_id.as_str() == call.id().as_str() {
                None
            } else {
                Some(IdMapping::new(call.id().clone(), normalized_id.clone()))
            };
            Ok((
                ToolCall::new(normalized_id, call.name().clone(), call.arguments().clone()),
                mapping,
            ))
        }
    }
}

enum SanitizeError {
    Collision,
    Empty,
}

fn sanitize_openai_tool_call_id(
    original: &ToolCallId,
    occupied: &mut BTreeSet<String>,
) -> Result<String, SanitizeError> {
    let filtered: String = original
        .as_str()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect();
    if (1..=40).contains(&filtered.len()) && !occupied.contains(&filtered) {
        occupied.insert(filtered.clone());
        return Ok(filtered);
    }

    let prefix = {
        let candidate: String = filtered.chars().take(27).collect();
        if candidate.is_empty() {
            "call".to_owned()
        } else {
            candidate
        }
    };
    let hash = fnv1a64(original.as_str().as_bytes());
    let candidate = format!("{prefix}_{:012x}", hash & 0xffff_ffff_ffff);
    if occupied.contains(&candidate) {
        return Err(SanitizeError::Collision);
    }
    if candidate.is_empty() {
        return Err(SanitizeError::Empty);
    }
    occupied.insert(candidate.clone());
    Ok(candidate)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x1000_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn resolve_mapped_id(
    original_id: &str,
    mappings: &[IdMapping],
    tool_calls: &[(ToolCallId, ToolName)],
) -> Result<ToolCallId, HistoryError> {
    if let Some(mapping) = mappings
        .iter()
        .find(|mapping| mapping.original().as_str() == original_id)
    {
        return Ok(mapping.normalized().clone());
    }
    // After a previous normalize pass, callers may already hold sanitized ids.
    if mappings
        .iter()
        .any(|mapping| mapping.normalized().as_str() == original_id)
        || tool_calls.iter().any(|(id, _)| id.as_str() == original_id)
    {
        return ToolCallId::new(original_id.to_owned()).map_err(|_| {
            HistoryError::new(
                "tool_call_id",
                HistoryFailure::UnknownToolCall,
                None,
                "tool result references an invalid tool call id",
            )
        });
    }
    Err(HistoryError::new(
        "tool_call_id",
        HistoryFailure::UnknownToolCall,
        None,
        "tool result references an unknown tool call id",
    ))
}

fn remap_tool_result(
    result: &ToolResultMessage,
    mapped_id: &ToolCallId,
    total_text_bytes: &mut usize,
    max_total_text_bytes: usize,
) -> Result<ToolResultMessage, HistoryError> {
    for part in result.content() {
        match part {
            ContentPart::Text { text } => {
                add_text_bytes(total_text_bytes, text.len(), max_total_text_bytes)?;
            }
            ContentPart::Image(_)
            | ContentPart::Thinking(_)
            | ContentPart::Refusal(_)
            | ContentPart::ToolCall(_) => {
                return Err(HistoryError::new(
                    "tool_result.content",
                    HistoryFailure::UnsupportedContent,
                    None,
                    "tool result content is not supported by the official profile",
                ));
            }
        }
    }
    ToolResultMessage::new(
        mapped_id.clone(),
        result.tool_name().clone(),
        result.content().to_vec(),
        result.is_error(),
        result.source_generation_id().cloned(),
    )
}

fn add_text_bytes(
    total: &mut usize,
    added: usize,
    max_total_text_bytes: usize,
) -> Result<(), HistoryError> {
    let next = total.checked_add(added).ok_or_else(|| {
        HistoryError::new(
            "messages",
            HistoryFailure::TextTooLarge,
            None,
            "history text size overflowed",
        )
    })?;
    if next > max_total_text_bytes {
        return Err(HistoryError::new(
            "messages",
            HistoryFailure::TextTooLarge,
            None,
            "history exceeds the allowed total text byte limit",
        ));
    }
    *total = next;
    Ok(())
}
