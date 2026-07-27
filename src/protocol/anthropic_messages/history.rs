use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

use crate::domain::content::decode_validated_data_url;
use crate::domain::{
    ContentPart, DiagnosticCode, ImageDetail, ImageSource, Message, MessageRole,
    NormalizationDiagnostic, SourceIdentity, ThinkingContent, ThinkingReplayPolicy,
};
use crate::error::{LlmError, ProtocolError};
use crate::plan::ResolvedCallPlan;

use super::wire::{
    ImageSourceWire, MessageRoleWire, MessageWire, RequestContentBlockWire, SystemBlockWire,
};

pub(super) struct AnthropicHistoryPlan {
    pub(super) system: Option<Vec<SystemBlockWire>>,
    pub(super) messages: Vec<MessageWire>,
    diagnostics: Vec<NormalizationDiagnostic>,
}

impl AnthropicHistoryPlan {
    pub(super) fn diagnostics(&self) -> &[NormalizationDiagnostic] {
        &self.diagnostics
    }
}

pub(super) fn plan_history(plan: &ResolvedCallPlan) -> Result<AnthropicHistoryPlan, LlmError> {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    let mut diagnostics = Vec::new();
    let mut image_count = 0usize;
    let target = plan.planned.source.clone();

    for message in &plan.planned.messages {
        match message.role() {
            MessageRole::System | MessageRole::Developer => {
                if message.role() == MessageRole::Developer {
                    increment_diagnostic(
                        &mut diagnostics,
                        DiagnosticCode::ConvertedDeveloperToSystem,
                    );
                }
                for part in message.content() {
                    let ContentPart::Text { text } = part else {
                        return Err(ProtocolError::new(
                            "Anthropic system content only accepts text blocks",
                        )
                        .into());
                    };
                    if text.is_empty() {
                        return Err(ProtocolError::new(
                            "Anthropic system text blocks must not be empty",
                        )
                        .into());
                    }
                    system.push(SystemBlockWire::text(text.clone()));
                }
            }
            MessageRole::User | MessageRole::Assistant => {
                let role = if message.role() == MessageRole::User {
                    MessageRoleWire::User
                } else {
                    MessageRoleWire::Assistant
                };
                let mut content = Vec::new();
                for part in message.content() {
                    if let Some(block) = encode_content(
                        part,
                        message.role(),
                        plan,
                        &target,
                        &mut diagnostics,
                        &mut image_count,
                    )? {
                        content.push(block);
                    }
                }
                if content.is_empty() {
                    if message.role() == MessageRole::Assistant
                        && message
                            .content()
                            .iter()
                            .all(|part| matches!(part, ContentPart::Thinking(_)))
                    {
                        continue;
                    }
                    return Err(
                        ProtocolError::new("Anthropic message content must not be empty").into(),
                    );
                }
                push_turn(&mut messages, role, content, &mut diagnostics);
            }
            MessageRole::Tool => {
                push_turn(
                    &mut messages,
                    MessageRoleWire::User,
                    vec![encode_tool_result(message)?],
                    &mut diagnostics,
                );
            }
        }
    }

    if messages.is_empty() || messages[0].role != MessageRoleWire::User {
        return Err(ProtocolError::new(
            "Anthropic Messages history must begin with a non-empty user turn",
        )
        .into());
    }

    Ok(AnthropicHistoryPlan {
        system: (!system.is_empty()).then_some(system),
        messages,
        diagnostics,
    })
}

fn encode_tool_result(message: &Message) -> Result<RequestContentBlockWire, LlmError> {
    let result = message
        .tool_result()
        .ok_or_else(|| ProtocolError::new("tool role message is missing its typed tool result"))?;
    let text = result
        .content()
        .first()
        .and_then(ContentPart::text_value)
        .ok_or_else(|| ProtocolError::new("Anthropic tool result requires text"))?;
    Ok(RequestContentBlockWire::ToolResult {
        tool_use_id: result.tool_call_id().as_str().to_owned(),
        content: text.to_owned(),
        is_error: result.is_error(),
    })
}

fn push_turn(
    messages: &mut Vec<MessageWire>,
    role: MessageRoleWire,
    mut content: Vec<RequestContentBlockWire>,
    diagnostics: &mut Vec<NormalizationDiagnostic>,
) {
    if let Some(previous) = messages.last_mut()
        && previous.role == role
    {
        previous.content.append(&mut content);
        increment_diagnostic(diagnostics, DiagnosticCode::MergedAdjacentMessages);
    } else {
        messages.push(MessageWire { role, content });
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_content(
    part: &ContentPart,
    role: MessageRole,
    plan: &ResolvedCallPlan,
    target: &SourceIdentity,
    diagnostics: &mut Vec<NormalizationDiagnostic>,
    image_count: &mut usize,
) -> Result<Option<RequestContentBlockWire>, LlmError> {
    match part {
        ContentPart::Text { text } => {
            if text.is_empty() {
                return Err(ProtocolError::new("Anthropic text blocks must not be empty").into());
            }
            Ok(Some(RequestContentBlockWire::Text { text: text.clone() }))
        }
        ContentPart::Image(image) if role == MessageRole::User => {
            if image.detail() != ImageDetail::Auto {
                return Err(ProtocolError::new(
                    "Anthropic Messages does not support image detail selection",
                )
                .into());
            }
            *image_count = image_count
                .checked_add(1)
                .ok_or_else(|| ProtocolError::new("Anthropic image count overflow"))?;
            if *image_count > plan.policy.limits.request.max_images {
                return Err(
                    ProtocolError::new("Anthropic image count exceeds resource limit").into(),
                );
            }
            let source = match image.source() {
                ImageSource::Url(url) => {
                    if url.as_str().len() > plan.policy.limits.request.max_image_url_bytes {
                        return Err(ProtocolError::new(
                            "Anthropic image URL exceeds resource limit",
                        )
                        .into());
                    }
                    ImageSourceWire::Url {
                        url: url.as_str().to_owned(),
                    }
                }
                ImageSource::Inline { mime, bytes } => {
                    if bytes.len() > plan.policy.limits.request.max_inline_image_bytes {
                        return Err(ProtocolError::new(
                            "Anthropic inline image exceeds resource limit",
                        )
                        .into());
                    }
                    ImageSourceWire::Base64 {
                        media_type: mime.as_str().to_owned(),
                        data: BASE64_STANDARD.encode(bytes),
                    }
                }
                ImageSource::DataUrl(data_url) => {
                    let (mime, bytes) = decode_validated_data_url(data_url)?;
                    if bytes.len() > plan.policy.limits.request.max_inline_image_bytes {
                        return Err(ProtocolError::new(
                            "Anthropic inline image exceeds resource limit",
                        )
                        .into());
                    }
                    ImageSourceWire::Base64 {
                        media_type: mime.as_str().to_owned(),
                        data: BASE64_STANDARD.encode(bytes),
                    }
                }
            };
            Ok(Some(RequestContentBlockWire::Image { source }))
        }
        ContentPart::Image(_) => {
            Err(ProtocolError::new("Anthropic image content is only valid in user messages").into())
        }
        ContentPart::ToolCall(call) if role == MessageRole::Assistant => {
            Ok(Some(RequestContentBlockWire::ToolUse {
                id: call.id().as_str().to_owned(),
                name: call.name().as_str().to_owned(),
                input: call.arguments().value().clone(),
            }))
        }
        ContentPart::ToolCall(_) => Err(ProtocolError::new(
            "Anthropic tool_use content is only valid in assistant messages",
        )
        .into()),
        ContentPart::Thinking(thinking) if role == MessageRole::Assistant => encode_thinking(
            thinking,
            plan.policy.history.thinking_replay,
            target,
            diagnostics,
        ),
        ContentPart::Thinking(_) => Err(ProtocolError::new(
            "Anthropic thinking content is only valid in assistant messages",
        )
        .into()),
        ContentPart::Refusal(_) => Err(ProtocolError::new(
            "Anthropic refusal replay is not supported by the current Domain contract",
        )
        .into()),
    }
}

fn encode_thinking(
    thinking: &ThinkingContent,
    policy: ThinkingReplayPolicy,
    target: &SourceIdentity,
    diagnostics: &mut Vec<NormalizationDiagnostic>,
) -> Result<Option<RequestContentBlockWire>, LlmError> {
    let Some(opaque) = thinking.opaque() else {
        increment_diagnostic(diagnostics, DiagnosticCode::DroppedThinkingOpaque);
        return Ok(None);
    };
    if policy != ThinkingReplayPolicy::SameSourceOnly || !opaque.source().matches_source(target) {
        increment_diagnostic(diagnostics, DiagnosticCode::DroppedThinkingOpaque);
        return Ok(None);
    }
    let value = std::str::from_utf8(opaque.bytes())
        .map_err(|_| ProtocolError::new("Anthropic opaque thinking must be valid UTF-8"))?
        .to_owned();
    if opaque.is_redacted() {
        if !thinking.text().is_empty() {
            return Err(ProtocolError::new(
                "redacted Anthropic thinking cannot carry visible text",
            )
            .into());
        }
        Ok(Some(RequestContentBlockWire::RedactedThinking {
            data: value,
        }))
    } else {
        Ok(Some(RequestContentBlockWire::Thinking {
            thinking: thinking.text().to_owned(),
            signature: value,
        }))
    }
}

fn increment_diagnostic(diagnostics: &mut Vec<NormalizationDiagnostic>, code: DiagnosticCode) {
    if let Some(index) = diagnostics.iter().position(|item| item.code() == code) {
        let count = diagnostics[index].count().saturating_add(1);
        diagnostics[index] = NormalizationDiagnostic::new(code, count);
    } else {
        diagnostics.push(NormalizationDiagnostic::new(code, 1));
    }
}
