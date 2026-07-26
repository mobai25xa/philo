use std::collections::BTreeMap;
use std::fmt;

use bytes::Bytes;
use serde::de::DeserializeOwned;

use super::stream::AnthropicMessagesStreamContext;
use crate::domain::{
    AssistantEvent, ContentIndex, FinishReason, GenerationId, OpaqueReasoning, ProtocolId,
    ResponseFormat, SchemaLimits, SourceIdentity, TokenCount, ToolArguments, ToolCall, ToolCallId,
    ToolName, UsageDetails, WireToolIndex,
};
use crate::error::{
    ErrorStage, LlmError, ProtocolError, RetriableHint, TruncatedStreamError, UnknownFinishReason,
    UnsupportedResponseSemantics,
};
use crate::provider::AnthropicUsageCompat;
use crate::provider::call_policy::ResponseLimits;
use crate::transport::SseEvent;

use super::super::wire::{
    ContentBlockDeltaEventWire, ContentBlockDeltaWire, ContentBlockStartEventWire,
    ContentBlockStartWire, ErrorEventWire, IndexedEventWire, MessageDeltaEventWire,
    MessageStartEventWire, TypeOnlyEventWire, UsageWire,
};

const MAX_CONTENT_BLOCKS: usize = 4096;
const MAX_STREAM_EVENTS: u64 = 100_000;
const MAX_OPAQUE_THINKING_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENT_TYPE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    AwaitMessageStart,
    StreamingMessage,
    AwaitMessageStop,
    Terminal,
}

enum BlockState {
    Text {
        index: ContentIndex,
    },
    ToolUse {
        index: ContentIndex,
        id: ToolCallId,
        name: ToolName,
        arguments: String,
    },
    Thinking {
        index: ContentIndex,
        signature: String,
    },
    Redacted {
        index: ContentIndex,
        data: String,
    },
    Unsupported,
}

impl fmt::Debug for BlockState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { index } => formatter
                .debug_struct("Text")
                .field("index", index)
                .finish(),
            Self::ToolUse {
                index, arguments, ..
            } => formatter
                .debug_struct("ToolUse")
                .field("index", index)
                .field("argument_bytes", &arguments.len())
                .finish_non_exhaustive(),
            Self::Thinking { index, signature } => formatter
                .debug_struct("Thinking")
                .field("index", index)
                .field("signature_bytes", &signature.len())
                .finish(),
            Self::Redacted { index, data } => formatter
                .debug_struct("Redacted")
                .field("index", index)
                .field("opaque_bytes", &data.len())
                .finish(),
            Self::Unsupported => formatter.write_str("Unsupported"),
        }
    }
}

pub(super) struct MessagesStateMachine {
    context: AnthropicMessagesStreamContext,
    response_format: ResponseFormat,
    limits: ResponseLimits,
    phase: Phase,
    blocks: BTreeMap<u32, BlockState>,
    blocks_started: usize,
    events_seen: u64,
    next_content_index: u32,
    tool_calls: usize,
    all_tool_argument_bytes: usize,
    text: String,
    thinking_text_bytes: usize,
    opaque_thinking_bytes: usize,
    usage_snapshot: Option<UsageWire>,
    usage_compat: AnthropicUsageCompat,
    usage_emitted: bool,
    finish_reason: Option<FinishReason>,
    generation_id: Option<GenerationId>,
}

impl fmt::Debug for MessagesStateMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessagesStateMachine")
            .field("phase", &self.phase)
            .field("open_blocks", &self.blocks.len())
            .field("blocks_started", &self.blocks_started)
            .field("events_seen", &self.events_seen)
            .field("next_content_index", &self.next_content_index)
            .field("tool_calls", &self.tool_calls)
            .field("tool_argument_bytes", &self.all_tool_argument_bytes)
            .field("text_bytes", &self.text.len())
            .field("thinking_text_bytes", &self.thinking_text_bytes)
            .field("opaque_thinking_bytes", &self.opaque_thinking_bytes)
            .field("usage_seen", &self.usage_snapshot.is_some())
            .field("usage_emitted", &self.usage_emitted)
            .field("finish_seen", &self.finish_reason.is_some())
            .field("generation_id_seen", &self.generation_id.is_some())
            .finish_non_exhaustive()
    }
}

impl MessagesStateMachine {
    pub(super) fn new(
        context: AnthropicMessagesStreamContext,
        response_format: ResponseFormat,
        limits: ResponseLimits,
        usage_compat: AnthropicUsageCompat,
    ) -> Self {
        Self {
            context,
            response_format,
            limits,
            phase: Phase::AwaitMessageStart,
            blocks: BTreeMap::new(),
            blocks_started: 0,
            events_seen: 0,
            next_content_index: 0,
            tool_calls: 0,
            all_tool_argument_bytes: 0,
            text: String::new(),
            thinking_text_bytes: 0,
            opaque_thinking_bytes: 0,
            usage_snapshot: None,
            usage_compat,
            usage_emitted: false,
            finish_reason: None,
            generation_id: None,
        }
    }

    fn protocol(message: impl Into<String>) -> LlmError {
        ProtocolError::at_stage(ErrorStage::Protocol, message).into()
    }

    pub(super) fn accept(&mut self, event: &SseEvent) -> Result<Vec<AssistantEvent>, LlmError> {
        self.events_seen = self
            .events_seen
            .checked_add(1)
            .ok_or_else(|| Self::protocol("Anthropic stream event count overflow"))?;
        if self.events_seen > MAX_STREAM_EVENTS {
            return Err(Self::protocol("Anthropic stream event limit exceeded"));
        }
        if self.phase == Phase::Terminal {
            return Err(Self::protocol(
                "Anthropic event received after message_stop",
            ));
        }
        let event_name = event
            .event_type()
            .ok_or_else(|| Self::protocol("Anthropic SSE event is missing its event name"))?;
        if event_name.len() > MAX_EVENT_TYPE_BYTES {
            return Err(Self::protocol(
                "Anthropic SSE event name exceeds resource limit",
            ));
        }
        let json_type = json_event_type(event.data())?;
        if event_name != json_type {
            return Err(Self::protocol(
                "Anthropic SSE event name does not match JSON event type",
            ));
        }

        match event_name {
            "message_start" => self.message_start(parse(event.data())?),
            "content_block_start" => self.block_start(parse(event.data())?),
            "content_block_delta" => self.block_delta(parse(event.data())?),
            "content_block_stop" => {
                let wire = parse(event.data())?;
                self.block_stop(&wire)
            }
            "message_delta" => self.message_delta(parse(event.data())?),
            "message_stop" => {
                let wire = parse(event.data())?;
                self.message_stop(&wire)
            }
            "ping" => {
                let wire: TypeOnlyEventWire = parse(event.data())?;
                validate_kind(&wire.kind, "ping")?;
                Ok(Vec::new())
            }
            "error" => {
                let wire: ErrorEventWire = parse(event.data())?;
                validate_kind(&wire.kind, "error")?;
                let kind = bounded_error_kind(&wire.error.kind)?;
                let hint = if kind == "overloaded_error" {
                    RetriableHint::Maybe
                } else {
                    RetriableHint::No
                };
                Err(ProtocolError::at_stage(
                    ErrorStage::Protocol,
                    "Anthropic stream returned a provider error",
                )
                .with_provider_context(
                    kind.to_owned(),
                    self.context.provider_request_id.clone(),
                    hint,
                )
                .into())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn message_start(
        &mut self,
        wire: MessageStartEventWire,
    ) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "message_start")?;
        if self.phase != Phase::AwaitMessageStart {
            return Err(Self::protocol("duplicate or late Anthropic message_start"));
        }
        if wire.message.model.is_empty() || wire.message.model.trim() != wire.message.model {
            return Err(Self::protocol(
                "invalid Anthropic response model identifier",
            ));
        }
        let generation_id = GenerationId::new(wire.message.id)
            .map_err(|_| Self::protocol("invalid Anthropic generation id"))?;
        self.generation_id = Some(generation_id.clone());
        self.usage_snapshot = Some(wire.message.usage);
        self.phase = Phase::StreamingMessage;
        Ok(vec![AssistantEvent::Start {
            local_request_id: self.context.local_request_id.clone(),
            provider_request_id: self.context.provider_request_id.clone(),
            generation_id: Some(generation_id),
        }])
    }

    fn block_start(
        &mut self,
        wire: ContentBlockStartEventWire,
    ) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "content_block_start")?;
        self.require_streaming("content_block_start")?;
        if self.blocks_started >= MAX_CONTENT_BLOCKS {
            return Err(Self::protocol("Anthropic content block limit exceeded"));
        }
        if self.blocks.contains_key(&wire.index) {
            return Err(Self::protocol("duplicate Anthropic content block index"));
        }

        let mut events = Vec::new();
        let block = match wire.content_block {
            ContentBlockStartWire::Text { text } => {
                let index = self.allocate_content_index()?;
                events.push(AssistantEvent::TextStart { index });
                if !text.is_empty() {
                    self.push_text(index, text, &mut events)?;
                }
                BlockState::Text { index }
            }
            ContentBlockStartWire::ToolUse { id, name, input } => {
                if self.tool_calls >= self.limits.max_tool_calls {
                    return Err(Self::protocol("Anthropic tool call limit exceeded"));
                }
                let id = ToolCallId::new(id)
                    .map_err(|_| Self::protocol("invalid Anthropic tool call id"))?;
                let name = ToolName::new(name)
                    .map_err(|_| Self::protocol("invalid Anthropic tool name"))?;
                let index = self.allocate_content_index()?;
                let wire_index = WireToolIndex::new(wire.index);
                events.push(AssistantEvent::ToolCallStart {
                    index,
                    wire_index,
                    id: Some(id.clone()),
                });
                events.push(AssistantEvent::ToolCallDelta {
                    index,
                    wire_index,
                    name_delta: Some(name.as_str().to_owned()),
                    arguments_delta: None,
                });
                let arguments = if input.as_object().is_some_and(serde_json::Map::is_empty) {
                    String::new()
                } else {
                    let initial = input.to_string();
                    events.push(AssistantEvent::ToolCallDelta {
                        index,
                        wire_index,
                        name_delta: None,
                        arguments_delta: Some(initial.clone()),
                    });
                    initial
                };
                self.tool_calls += 1;
                self.all_tool_argument_bytes = self
                    .all_tool_argument_bytes
                    .checked_add(arguments.len())
                    .ok_or_else(|| Self::protocol("Anthropic tool argument size overflow"))?;
                if arguments.len() > self.limits.max_tool_arguments_bytes
                    || self.all_tool_argument_bytes > self.limits.max_all_tool_arguments_bytes
                {
                    return Err(Self::protocol(
                        "Anthropic tool arguments exceed resource limit",
                    ));
                }
                BlockState::ToolUse {
                    index,
                    id,
                    name,
                    arguments,
                }
            }
            ContentBlockStartWire::Thinking { thinking } => {
                let index = self.allocate_content_index()?;
                events.push(AssistantEvent::ThinkingStart { index });
                if !thinking.is_empty() {
                    self.push_thinking(index, thinking, &mut events)?;
                }
                BlockState::Thinking {
                    index,
                    signature: String::new(),
                }
            }
            ContentBlockStartWire::RedactedThinking { data } => {
                self.reserve_opaque_thinking(data.len())?;
                let index = self.allocate_content_index()?;
                events.push(AssistantEvent::ThinkingStart { index });
                BlockState::Redacted { index, data }
            }
            ContentBlockStartWire::Unknown => BlockState::Unsupported,
        };
        self.blocks.insert(wire.index, block);
        self.blocks_started += 1;
        Ok(events)
    }

    fn block_delta(
        &mut self,
        wire: ContentBlockDeltaEventWire,
    ) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "content_block_delta")?;
        self.require_streaming("content_block_delta")?;
        let Some(block) = self.blocks.get_mut(&wire.index) else {
            return Err(Self::protocol(
                "Anthropic content delta requires a matching block start",
            ));
        };
        let mut events = Vec::new();
        match (block, wire.delta) {
            (BlockState::Text { index }, ContentBlockDeltaWire::TextDelta { text }) => {
                let index = *index;
                self.push_text(index, text, &mut events)?;
            }
            (
                BlockState::ToolUse {
                    index, arguments, ..
                },
                ContentBlockDeltaWire::InputJsonDelta { partial_json },
            ) => {
                let next_one = arguments
                    .len()
                    .checked_add(partial_json.len())
                    .ok_or_else(|| Self::protocol("Anthropic tool argument size overflow"))?;
                let next_all = self
                    .all_tool_argument_bytes
                    .checked_add(partial_json.len())
                    .ok_or_else(|| Self::protocol("Anthropic tool argument size overflow"))?;
                if next_one > self.limits.max_tool_arguments_bytes
                    || next_all > self.limits.max_all_tool_arguments_bytes
                {
                    return Err(Self::protocol(
                        "Anthropic tool arguments exceed resource limit",
                    ));
                }
                arguments.push_str(&partial_json);
                self.all_tool_argument_bytes = next_all;
                events.push(AssistantEvent::ToolCallDelta {
                    index: *index,
                    wire_index: WireToolIndex::new(wire.index),
                    name_delta: None,
                    arguments_delta: Some(partial_json),
                });
            }
            (
                BlockState::Thinking { index, .. },
                ContentBlockDeltaWire::ThinkingDelta { thinking },
            ) => {
                let index = *index;
                self.push_thinking(index, thinking, &mut events)?;
            }
            (
                BlockState::Thinking { signature, .. },
                ContentBlockDeltaWire::SignatureDelta { signature: delta },
            ) => {
                let next = signature
                    .len()
                    .checked_add(delta.len())
                    .ok_or_else(|| Self::protocol("Anthropic thinking signature size overflow"))?;
                if next > MAX_OPAQUE_THINKING_BYTES {
                    return Err(Self::protocol(
                        "Anthropic thinking signature exceeds resource limit",
                    ));
                }
                let opaque_total = self
                    .opaque_thinking_bytes
                    .checked_add(delta.len())
                    .ok_or_else(|| Self::protocol("Anthropic opaque thinking size overflow"))?;
                if opaque_total > MAX_OPAQUE_THINKING_BYTES {
                    return Err(Self::protocol(
                        "Anthropic opaque thinking exceeds resource limit",
                    ));
                }
                self.opaque_thinking_bytes = opaque_total;
                signature.push_str(&delta);
            }
            (BlockState::Unsupported, _) => {}
            (_, ContentBlockDeltaWire::Unknown) => {
                return Err(Self::protocol(
                    "unknown Anthropic delta cannot modify a known content block",
                ));
            }
            _ => {
                return Err(Self::protocol(
                    "Anthropic content delta type does not match its open block",
                ));
            }
        }
        Ok(events)
    }

    fn block_stop(&mut self, wire: &IndexedEventWire) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "content_block_stop")?;
        self.require_streaming("content_block_stop")?;
        let Some(block) = self.blocks.remove(&wire.index) else {
            return Err(Self::protocol(
                "Anthropic content block stop requires a matching start",
            ));
        };
        match block {
            BlockState::Text { index } => Ok(vec![AssistantEvent::TextEnd { index }]),
            BlockState::Thinking { index, signature } => {
                let mut events = Vec::new();
                if !signature.is_empty() {
                    events.push(AssistantEvent::ThinkingOpaque {
                        index,
                        opaque: OpaqueReasoning::new(
                            Bytes::from(signature),
                            self.source_identity()?,
                            false,
                        ),
                    });
                }
                events.push(AssistantEvent::ThinkingEnd { index });
                Ok(events)
            }
            BlockState::ToolUse {
                index,
                id,
                name,
                arguments,
            } => {
                let arguments = if arguments.is_empty() {
                    "{}".to_owned()
                } else {
                    arguments
                };
                let arguments = ToolArguments::from_raw_json(arguments).map_err(|_| {
                    Self::protocol("Anthropic tool input is not complete valid JSON")
                })?;
                if !arguments.value().is_object() {
                    return Err(Self::protocol("Anthropic tool input must be a JSON object"));
                }
                Ok(vec![AssistantEvent::ToolCallEnd {
                    index,
                    call: ToolCall::new(id, name, arguments),
                }])
            }
            BlockState::Redacted { index, data } => Ok(vec![
                AssistantEvent::ThinkingOpaque {
                    index,
                    opaque: OpaqueReasoning::new(Bytes::from(data), self.source_identity()?, true),
                },
                AssistantEvent::ThinkingEnd { index },
            ]),
            BlockState::Unsupported => Ok(Vec::new()),
        }
    }

    fn message_delta(
        &mut self,
        wire: MessageDeltaEventWire,
    ) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "message_delta")?;
        self.require_streaming("message_delta")?;
        if !self.blocks.is_empty() {
            return Err(Self::protocol(
                "Anthropic message_delta received before all content blocks stopped",
            ));
        }
        let merged_usage = merge_usage_snapshot(
            self.usage_snapshot.unwrap_or_default(),
            wire.usage,
            self.usage_compat,
        )?;
        self.usage_snapshot = Some(merged_usage);
        let Some(raw_reason) = wire.delta.stop_reason else {
            return Ok(Vec::new());
        };
        let reason = match raw_reason.as_str() {
            "end_turn" | "stop_sequence" => FinishReason::Stop,
            "max_tokens" => FinishReason::Length,
            "tool_use" => FinishReason::ToolCalls,
            "pause_turn" => return Err(UnsupportedResponseSemantics::new("pause_turn").into()),
            "refusal" | "model_context_window_exceeded" => {
                return Err(UnknownFinishReason::new(raw_reason).into());
            }
            _ => return Err(UnknownFinishReason::new(bounded_finish_reason(&raw_reason)?).into()),
        };
        if matches!(reason, FinishReason::ToolCalls) && self.tool_calls == 0 {
            return Err(Self::protocol(
                "Anthropic tool_use stop reason requires a completed tool block",
            ));
        }
        self.finish_reason = Some(reason);
        self.phase = Phase::AwaitMessageStop;

        let details = usage_details(merged_usage)?;
        self.usage_emitted = details.has_any_known();
        Ok(if details.has_any_known() {
            vec![AssistantEvent::DetailedUsage(details)]
        } else {
            Vec::new()
        })
    }

    fn message_stop(&mut self, wire: &TypeOnlyEventWire) -> Result<Vec<AssistantEvent>, LlmError> {
        validate_kind(&wire.kind, "message_stop")?;
        if self.phase != Phase::AwaitMessageStop {
            return Err(Self::protocol(
                "Anthropic message_stop requires a completed message_delta",
            ));
        }
        let reason = self
            .finish_reason
            .clone()
            .ok_or_else(|| Self::protocol("Anthropic message_stop is missing a finish reason"))?;
        crate::domain::structured::validate_structured_response(
            &self.response_format,
            &reason,
            &self.text,
            self.tool_calls > 0,
            false,
            SchemaLimits::official(),
        )?;
        self.phase = Phase::Terminal;
        Ok(Vec::new())
    }

    fn push_text(
        &mut self,
        index: ContentIndex,
        text: String,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        let next = self
            .text
            .len()
            .checked_add(text.len())
            .ok_or_else(|| Self::protocol("Anthropic text size overflow"))?;
        if next > self.limits.max_structured_output_bytes {
            return Err(Self::protocol(
                "Anthropic response text exceeds resource limit",
            ));
        }
        self.text.push_str(&text);
        if !text.is_empty() {
            events.push(AssistantEvent::TextDelta { index, delta: text });
        }
        Ok(())
    }

    fn push_thinking(
        &mut self,
        index: ContentIndex,
        thinking: String,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        let next = self
            .thinking_text_bytes
            .checked_add(thinking.len())
            .ok_or_else(|| Self::protocol("Anthropic thinking text size overflow"))?;
        if next > self.limits.max_structured_output_bytes {
            return Err(Self::protocol(
                "Anthropic thinking text exceeds resource limit",
            ));
        }
        self.thinking_text_bytes = next;
        if !thinking.is_empty() {
            events.push(AssistantEvent::ThinkingDelta {
                index,
                delta: thinking,
            });
        }
        Ok(())
    }

    fn reserve_opaque_thinking(&mut self, bytes: usize) -> Result<(), LlmError> {
        let next = self
            .opaque_thinking_bytes
            .checked_add(bytes)
            .ok_or_else(|| Self::protocol("Anthropic opaque thinking size overflow"))?;
        if next > MAX_OPAQUE_THINKING_BYTES {
            return Err(Self::protocol(
                "Anthropic opaque thinking exceeds resource limit",
            ));
        }
        self.opaque_thinking_bytes = next;
        Ok(())
    }

    fn allocate_content_index(&mut self) -> Result<ContentIndex, LlmError> {
        let current = self.next_content_index;
        self.next_content_index = self
            .next_content_index
            .checked_add(1)
            .ok_or_else(|| Self::protocol("Anthropic content index overflow"))?;
        Ok(ContentIndex::new(current))
    }

    fn source_identity(&self) -> Result<SourceIdentity, LlmError> {
        let generation_id = self
            .generation_id
            .clone()
            .ok_or_else(|| Self::protocol("Anthropic thinking source omitted generation id"))?;
        let protocol = ProtocolId::new("anthropic-messages")
            .map_err(|_| Self::protocol("invalid Anthropic protocol identity"))?;
        Ok(SourceIdentity::new(
            self.context.source.provider().clone(),
            self.context.source.model().clone(),
            protocol,
        )
        .with_generation_id(generation_id))
    }

    fn require_streaming(&self, event: &str) -> Result<(), LlmError> {
        if self.phase == Phase::StreamingMessage {
            Ok(())
        } else {
            Err(Self::protocol(format!(
                "Anthropic {event} is invalid in the current message phase"
            )))
        }
    }

    pub(super) fn finish(&self) -> Result<Vec<AssistantEvent>, LlmError> {
        if self.phase == Phase::Terminal {
            Ok(vec![AssistantEvent::Done {
                finish_reason: self.finish_reason.clone().ok_or_else(|| {
                    Self::protocol("Anthropic terminal is missing a finish reason")
                })?,
            }])
        } else {
            Err(TruncatedStreamError.into())
        }
    }
}

fn json_event_type(data: &str) -> Result<String, LlmError> {
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|error| json_error(&error))?;
    value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| MessagesStateMachine::protocol("Anthropic event JSON omitted type"))
}

fn parse<T: DeserializeOwned>(data: &str) -> Result<T, LlmError> {
    serde_json::from_str(data).map_err(|error| json_error(&error))
}

fn json_error(error: &serde_json::Error) -> LlmError {
    ProtocolError::at_stage(
        ErrorStage::Json,
        format!(
            "Anthropic event JSON error at line {} column {}",
            error.line(),
            error.column()
        ),
    )
    .into()
}

fn validate_kind(actual: &str, expected: &str) -> Result<(), LlmError> {
    if actual == expected {
        Ok(())
    } else {
        Err(MessagesStateMachine::protocol(
            "Anthropic decoded event type does not match expected event",
        ))
    }
}

fn bounded_error_kind(value: &str) -> Result<&str, LlmError> {
    if value.is_empty()
        || value.len() > MAX_EVENT_TYPE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Err(MessagesStateMachine::protocol(
            "invalid Anthropic stream error type",
        ))
    } else {
        Ok(value)
    }
}

fn usage_details(snapshot: UsageWire) -> Result<UsageDetails, LlmError> {
    let input = match (
        snapshot.input_tokens,
        snapshot.cache_creation_input_tokens,
        snapshot.cache_read_input_tokens,
    ) {
        (Some(input), Some(write), Some(read)) => input
            .checked_add(write)
            .and_then(|value| value.checked_add(read))
            .map(TokenCount::Known)
            .ok_or_else(|| MessagesStateMachine::protocol("Anthropic input usage overflow"))?,
        _ => TokenCount::Unknown,
    };
    let details = UsageDetails::new(
        input,
        snapshot
            .output_tokens
            .map_or(TokenCount::Unknown, TokenCount::Known),
        TokenCount::Unknown,
        snapshot
            .cache_read_input_tokens
            .map_or(TokenCount::Unknown, TokenCount::Known),
        snapshot
            .cache_creation_input_tokens
            .map_or(TokenCount::Unknown, TokenCount::Known),
        snapshot
            .thinking_tokens
            .map_or(TokenCount::Unknown, TokenCount::Known),
    );
    details
        .validate_relationships()
        .map_err(|_| MessagesStateMachine::protocol("inconsistent Anthropic usage snapshot"))?;
    Ok(details)
}

fn merge_usage_snapshot(
    previous: UsageWire,
    next: UsageWire,
    compat: AnthropicUsageCompat,
) -> Result<UsageWire, LlmError> {
    Ok(UsageWire {
        input_tokens: merge_stable_count(previous.input_tokens, next.input_tokens, compat)?,
        output_tokens: merge_cumulative_count(previous.output_tokens, next.output_tokens)?,
        cache_creation_input_tokens: merge_stable_count(
            previous.cache_creation_input_tokens,
            next.cache_creation_input_tokens,
            compat,
        )?,
        cache_read_input_tokens: merge_stable_count(
            previous.cache_read_input_tokens,
            next.cache_read_input_tokens,
            compat,
        )?,
        thinking_tokens: merge_cumulative_count(previous.thinking_tokens, next.thinking_tokens)?,
    })
}

fn merge_stable_count(
    previous: Option<u64>,
    next: Option<u64>,
    compat: AnthropicUsageCompat,
) -> Result<Option<u64>, LlmError> {
    match (previous, next) {
        (Some(left), Some(right))
            if right > left
                && matches!(compat, AnthropicUsageCompat::AllowMonotonicStableFields) =>
        {
            Ok(Some(right))
        }
        (Some(left), Some(right)) if left != right => Err(MessagesStateMachine::protocol(
            "Anthropic stable usage field changed within one stream",
        )),
        (Some(value), _) | (None, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn merge_cumulative_count(
    previous: Option<u64>,
    next: Option<u64>,
) -> Result<Option<u64>, LlmError> {
    match (previous, next) {
        (Some(left), Some(right)) if right < left => Err(MessagesStateMachine::protocol(
            "Anthropic cumulative usage field decreased within one stream",
        )),
        (Some(_) | None, Some(right)) => Ok(Some(right)),
        (Some(value), None) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn bounded_finish_reason(value: &str) -> Result<String, LlmError> {
    if value.is_empty()
        || value.len() > MAX_EVENT_TYPE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err(MessagesStateMachine::protocol(
            "invalid Anthropic stop reason label",
        ))
    } else {
        Ok(value.to_owned())
    }
}
