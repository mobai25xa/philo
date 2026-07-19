//! Streaming assistant events and the single completion collector.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::struct_excessive_bools,
    clippy::struct_field_names
)]

use std::collections::{BTreeMap, BTreeSet};

use futures_util::{Stream, StreamExt};

pub use super::ids::{GenerationId, LocalRequestId, ProviderRequestId};
use super::schema::SchemaLimits;
use super::structured::ResponseFormat;
use super::usage::UsageDetails;
use super::{
    ContentIndex, ContentPart, ModelRef, RefusalContent, ThinkingContent, ToolCall, ToolCallId,
    WireToolIndex,
};
use crate::error::{
    LlmError, ProtocolError, StructuredOutputError, StructuredOutputFailure, TruncatedStreamError,
};

/// Token accounting supplied by a provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}
impl Usage {
    /// Creates usage and verifies total consistency.
    pub fn new(
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
    ) -> Result<Self, ProtocolError> {
        if input_tokens.checked_add(output_tokens) != Some(total_tokens) {
            return Err(ProtocolError::new(
                "usage total does not equal input + output",
            ));
        }
        Ok(Self {
            input_tokens,
            output_tokens,
            total_tokens,
        })
    }
    /// Input token count.
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }
    /// Output token count.
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }
    /// Total token count.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }
}

/// Normalized completion reason.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FinishReason {
    /// Natural completion.
    Stop,
    /// Output limit reached.
    Length,
    /// Provider content filter stopped generation.
    ContentFilter,
    /// One or more completed tool calls are available.
    ToolCalls,
    /// Provider-specific value retained without claiming a known success reason.
    Unknown(String),
}

/// A public event emitted by a protocol state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AssistantEvent {
    /// Identifies the local request and any provider generation IDs known at start.
    Start {
        /// Identifier allocated locally for this request attempt.
        local_request_id: LocalRequestId,
        /// Identifier returned in provider response headers, when available.
        provider_request_id: Option<ProviderRequestId>,
        /// Identifier returned in the generation body, when available.
        generation_id: Option<GenerationId>,
    },
    /// Starts a text content block.
    TextStart {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Appends a Unicode text fragment.
    TextDelta {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Unmodified text fragment.
        delta: String,
    },
    /// Ends a text content block.
    TextEnd {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Starts a visible thinking content block.
    ThinkingStart {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Appends a visible thinking fragment.
    ThinkingDelta {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Unmodified visible fragment.
        delta: String,
    },
    /// Ends a visible thinking content block.
    ThinkingEnd {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Starts a refusal content block.
    RefusalStart {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Appends a refusal fragment.
    RefusalDelta {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Unmodified refusal fragment.
        delta: String,
    },
    /// Ends a refusal content block.
    RefusalEnd {
        /// Provider-independent content position.
        index: ContentIndex,
    },
    /// Starts one tool call accumulator.
    ToolCallStart {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Protocol-local tool-call index.
        wire_index: WireToolIndex,
        /// Stable call ID when the protocol supplied it at start.
        id: Option<ToolCallId>,
    },
    /// Appends tool name or argument fragments.
    ToolCallDelta {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Protocol-local tool-call index.
        wire_index: WireToolIndex,
        /// Name fragment, when present in this event.
        name_delta: Option<String>,
        /// Raw JSON fragment, when present in this event.
        arguments_delta: Option<String>,
    },
    /// Ends a tool call only after its ID, name, and JSON are complete.
    ToolCallEnd {
        /// Provider-independent content position.
        index: ContentIndex,
        /// Completed call safe for later schema validation.
        call: ToolCall,
    },
    /// Reports complete P1 core token usage; absence means unknown.
    Usage(Usage),
    /// Reports detailed token usage, including optional/incomplete fields.
    DetailedUsage(UsageDetails),
    /// Ends the generation exactly once.
    Done {
        /// Normalized completion reason.
        finish_reason: FinishReason,
    },
}

impl AssistantEvent {
    /// Creates a Start event when only the local request id is known.
    pub fn start(local_request_id: LocalRequestId) -> Self {
        Self::Start {
            local_request_id,
            provider_request_id: None,
            generation_id: None,
        }
    }
}

/// The collected assistant result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistantMessage {
    content: Vec<ContentPart>,
    text: String,
    usage: Option<Usage>,
    usage_details: Option<UsageDetails>,
    structured_output: Option<serde_json::Value>,
    finish_reason: FinishReason,
    local_request_id: Option<LocalRequestId>,
    provider_request_id: Option<ProviderRequestId>,
    generation_id: Option<GenerationId>,
    model: Option<ModelRef>,
}
impl AssistantMessage {
    /// Returns all collected content in stable `ContentIndex` order.
    pub fn content(&self) -> &[ContentPart] {
        &self.content
    }

    /// Returns all text blocks concatenated in content order.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns usage, or `None` when the provider omitted complete core counters.
    pub fn usage(&self) -> Option<Usage> {
        self.usage
    }

    /// Returns detailed usage when any usage snapshot was observed.
    pub fn usage_details(&self) -> Option<UsageDetails> {
        self.usage_details
    }

    /// Returns validated structured output when requested and successful.
    pub fn structured_output(&self) -> Option<&serde_json::Value> {
        self.structured_output.as_ref()
    }

    /// Returns finish reason.
    pub fn finish_reason(&self) -> &FinishReason {
        &self.finish_reason
    }

    /// Returns local request ID.
    pub fn local_request_id(&self) -> Option<&LocalRequestId> {
        self.local_request_id.as_ref()
    }

    /// Returns provider request ID.
    pub fn provider_request_id(&self) -> Option<&ProviderRequestId> {
        self.provider_request_id.as_ref()
    }

    /// Returns generation ID.
    pub fn generation_id(&self) -> Option<&GenerationId> {
        self.generation_id.as_ref()
    }

    /// Returns selected model when attached by a higher layer.
    pub fn model(&self) -> Option<&ModelRef> {
        self.model.as_ref()
    }

    /// Attaches model context without changing collected semantics.
    pub fn with_model(mut self, model: ModelRef) -> Self {
        self.model = Some(model);
        self
    }
}

/// Collects one stream into an assistant message. It never performs a request.
///
/// This is equivalent to [`collect_assistant_message_for_format`] with
/// [`ResponseFormat::Text`].
pub async fn collect_assistant_message<S>(stream: S) -> Result<AssistantMessage, LlmError>
where
    S: Stream<Item = Result<AssistantEvent, LlmError>>,
{
    collect_assistant_message_for_format(stream, &ResponseFormat::Text).await
}

/// Collects one stream and validates structured output according to `response_format`.
///
/// Validation happens after the stream ends successfully. Intermediate text deltas
/// are not schema-validated. This function never performs a network request.
pub async fn collect_assistant_message_for_format<S>(
    stream: S,
    response_format: &ResponseFormat,
) -> Result<AssistantMessage, LlmError>
where
    S: Stream<Item = Result<AssistantEvent, LlmError>>,
{
    let mut stream = Box::pin(stream);
    let mut state = Collector::default();
    while let Some(item) = stream.next().await {
        state.accept(item?)?;
    }
    let mut message = state.finish().map_err(LlmError::from)?;
    message.structured_output = validate_structured_output(&message, response_format)?;
    Ok(message)
}

fn validate_structured_output(
    message: &AssistantMessage,
    response_format: &ResponseFormat,
) -> Result<Option<serde_json::Value>, LlmError> {
    match response_format {
        ResponseFormat::Text => Ok(None),
        ResponseFormat::JsonObject | ResponseFormat::JsonSchema(_) => {
            let has_tool_call = message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::ToolCall(_)));
            let has_refusal = message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Refusal(_)));
            if matches!(message.finish_reason, FinishReason::ToolCalls) || has_tool_call {
                return Ok(None);
            }
            if has_refusal {
                return Ok(None);
            }
            if matches!(message.finish_reason, FinishReason::Length) {
                return Err(StructuredOutputError::new(
                    "structured_output",
                    StructuredOutputFailure::Truncated,
                    None,
                    "structured output was truncated before completion",
                )
                .into());
            }
            if matches!(
                message.finish_reason,
                FinishReason::ContentFilter | FinishReason::Unknown(_)
            ) {
                return Err(ProtocolError::new(
                    "structured output is unavailable for a non-success finish reason",
                )
                .into());
            }
            if !matches!(message.finish_reason, FinishReason::Stop) {
                return Err(ProtocolError::new(
                    "structured output requires a successful text finish",
                )
                .into());
            }

            let parsed =
                serde_json::from_str::<serde_json::Value>(message.text()).map_err(|_| {
                    StructuredOutputError::new(
                        "structured_output",
                        StructuredOutputFailure::InvalidJson,
                        None,
                        "assistant text is not valid JSON",
                    )
                })?;

            match response_format {
                ResponseFormat::JsonObject => {
                    if !parsed.is_object() {
                        return Err(StructuredOutputError::new(
                            "structured_output",
                            StructuredOutputFailure::SchemaViolation,
                            Some("#".to_owned()),
                            "json_object response must be a JSON object",
                        )
                        .into());
                    }
                    Ok(Some(parsed))
                }
                ResponseFormat::JsonSchema(schema) => {
                    schema
                        .schema()
                        .validate_instance(&parsed, SchemaLimits::official())
                        .map_err(|error| {
                            StructuredOutputError::new(
                                "structured_output",
                                StructuredOutputFailure::SchemaViolation,
                                error.path().map(str::to_owned),
                                "assistant text failed the requested response schema",
                            )
                        })?;
                    Ok(Some(parsed))
                }
                ResponseFormat::Text => Ok(None),
            }
        }
    }
}

enum OpenBlock {
    Text(String),
    Thinking(String),
    Refusal(String),
    ToolCall {
        wire_index: WireToolIndex,
        id: Option<ToolCallId>,
        name: String,
        arguments: String,
    },
}

#[derive(Default)]
struct Collector {
    started: bool,
    done: bool,
    next_content_index: u32,
    open: BTreeMap<ContentIndex, OpenBlock>,
    content: BTreeMap<ContentIndex, ContentPart>,
    tool_wire_indexes: BTreeSet<WireToolIndex>,
    tool_call_ids: BTreeSet<ToolCallId>,
    usage: Option<Usage>,
    usage_details: Option<UsageDetails>,
    finish_reason: Option<FinishReason>,
    local_request_id: Option<LocalRequestId>,
    provider_request_id: Option<ProviderRequestId>,
    generation_id: Option<GenerationId>,
}

impl Collector {
    fn protocol(message: impl Into<String>) -> LlmError {
        ProtocolError::new(message).into()
    }

    #[allow(clippy::too_many_lines)]
    fn accept(&mut self, event: AssistantEvent) -> Result<(), LlmError> {
        if self.done {
            return Err(Self::protocol("event received after Done"));
        }
        match event {
            AssistantEvent::Start {
                local_request_id,
                provider_request_id,
                generation_id,
            } => {
                if self.started || self.next_content_index != 0 {
                    return Err(Self::protocol("duplicate or late Start"));
                }
                self.started = true;
                self.local_request_id = Some(local_request_id);
                self.provider_request_id = provider_request_id;
                self.generation_id = generation_id;
            }
            AssistantEvent::TextStart { index } => {
                self.start_block(index, OpenBlock::Text(String::new()))?;
            }
            AssistantEvent::TextDelta { index, delta } => {
                let Some(OpenBlock::Text(text)) = self.open.get_mut(&index) else {
                    return Err(Self::protocol("TextDelta requires a matching TextStart"));
                };
                text.push_str(&delta);
            }
            AssistantEvent::TextEnd { index } => {
                let Some(OpenBlock::Text(text)) = self.open.remove(&index) else {
                    return Err(Self::protocol("TextEnd requires a matching TextStart"));
                };
                self.end_block(index, ContentPart::text(text))?;
            }
            AssistantEvent::ThinkingStart { index } => {
                self.start_block(index, OpenBlock::Thinking(String::new()))?;
            }
            AssistantEvent::ThinkingDelta { index, delta } => {
                let Some(OpenBlock::Thinking(text)) = self.open.get_mut(&index) else {
                    return Err(Self::protocol(
                        "ThinkingDelta requires a matching ThinkingStart",
                    ));
                };
                text.push_str(&delta);
            }
            AssistantEvent::ThinkingEnd { index } => {
                let Some(OpenBlock::Thinking(text)) = self.open.remove(&index) else {
                    return Err(Self::protocol(
                        "ThinkingEnd requires a matching ThinkingStart",
                    ));
                };
                self.end_block(index, ContentPart::Thinking(ThinkingContent::new(text)))?;
            }
            AssistantEvent::RefusalStart { index } => {
                self.start_block(index, OpenBlock::Refusal(String::new()))?;
            }
            AssistantEvent::RefusalDelta { index, delta } => {
                let Some(OpenBlock::Refusal(text)) = self.open.get_mut(&index) else {
                    return Err(Self::protocol(
                        "RefusalDelta requires a matching RefusalStart",
                    ));
                };
                text.push_str(&delta);
            }
            AssistantEvent::RefusalEnd { index } => {
                let Some(OpenBlock::Refusal(text)) = self.open.remove(&index) else {
                    return Err(Self::protocol(
                        "RefusalEnd requires a matching RefusalStart",
                    ));
                };
                self.end_block(index, ContentPart::Refusal(RefusalContent::new(text)))?;
            }
            AssistantEvent::ToolCallStart {
                index,
                wire_index,
                id,
            } => {
                if !self.tool_wire_indexes.insert(wire_index) {
                    return Err(Self::protocol("duplicate tool wire index"));
                }
                if id
                    .as_ref()
                    .is_some_and(|id| !self.tool_call_ids.insert(id.clone()))
                {
                    return Err(Self::protocol("duplicate tool call id"));
                }
                self.start_block(
                    index,
                    OpenBlock::ToolCall {
                        wire_index,
                        id,
                        name: String::new(),
                        arguments: String::new(),
                    },
                )?;
            }
            AssistantEvent::ToolCallDelta {
                index,
                wire_index,
                name_delta,
                arguments_delta,
            } => {
                let Some(OpenBlock::ToolCall {
                    wire_index: expected_wire_index,
                    name,
                    arguments,
                    ..
                }) = self.open.get_mut(&index)
                else {
                    return Err(Self::protocol(
                        "ToolCallDelta requires a matching ToolCallStart",
                    ));
                };
                if *expected_wire_index != wire_index {
                    return Err(Self::protocol("ToolCallDelta wire index changed"));
                }
                if let Some(delta) = name_delta {
                    name.push_str(&delta);
                }
                if let Some(delta) = arguments_delta {
                    arguments.push_str(&delta);
                }
            }
            AssistantEvent::ToolCallEnd { index, call } => {
                let Some(OpenBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                }) = self.open.remove(&index)
                else {
                    return Err(Self::protocol(
                        "ToolCallEnd requires a matching ToolCallStart",
                    ));
                };
                if id.as_ref().is_some_and(|id| id != call.id()) {
                    return Err(Self::protocol("ToolCallEnd changed the tool call id"));
                }
                if id.is_none() && !self.tool_call_ids.insert(call.id().clone()) {
                    return Err(Self::protocol("duplicate tool call id"));
                }
                if name != call.name().as_str() {
                    return Err(Self::protocol("ToolCallEnd changed the tool name"));
                }
                if arguments != call.arguments().raw_json() {
                    return Err(Self::protocol("ToolCallEnd changed tool arguments"));
                }
                self.end_block(index, ContentPart::ToolCall(call))?;
            }
            AssistantEvent::Usage(usage) => {
                if let Some(previous) = self.usage {
                    if previous != usage {
                        return Err(Self::protocol("conflicting Usage events"));
                    }
                } else {
                    self.usage = Some(usage);
                }
            }
            AssistantEvent::DetailedUsage(details) => {
                if let Some(previous) = self.usage_details {
                    if previous != details {
                        return Err(Self::protocol("conflicting DetailedUsage events"));
                    }
                } else {
                    self.usage_details = Some(details);
                }
            }
            AssistantEvent::Done { finish_reason } => {
                if !self.open.is_empty() {
                    return Err(Self::protocol(
                        "Done received with an incomplete content block",
                    ));
                }
                if self.content.is_empty() {
                    return Err(Self::protocol(
                        "Done requires at least one completed content block",
                    ));
                }
                let has_tool_call = self
                    .content
                    .values()
                    .any(|part| matches!(part, ContentPart::ToolCall(_)));
                if matches!(finish_reason, FinishReason::ToolCalls) != has_tool_call {
                    return Err(Self::protocol(
                        "ToolCalls finish reason and collected tool calls disagree",
                    ));
                }
                self.done = true;
                self.finish_reason = Some(finish_reason);
            }
        }
        Ok(())
    }

    fn start_block(&mut self, index: ContentIndex, block: OpenBlock) -> Result<(), LlmError> {
        let expected = ContentIndex::new(self.next_content_index);
        if index != expected || self.open.contains_key(&index) || self.content.contains_key(&index)
        {
            return Err(Self::protocol(
                "content blocks must start once in contiguous ContentIndex order",
            ));
        }
        self.next_content_index = self
            .next_content_index
            .checked_add(1)
            .ok_or_else(|| Self::protocol("content index overflow"))?;
        self.open.insert(index, block);
        Ok(())
    }

    fn end_block(&mut self, index: ContentIndex, part: ContentPart) -> Result<(), LlmError> {
        if self.content.insert(index, part).is_some() {
            return Err(Self::protocol("duplicate content block end"));
        }
        Ok(())
    }

    fn finish(self) -> Result<AssistantMessage, TruncatedStreamError> {
        if !self.done {
            return Err(TruncatedStreamError);
        }
        let Some(finish_reason) = self.finish_reason else {
            return Err(TruncatedStreamError);
        };
        let content: Vec<_> = self.content.into_values().collect();
        let text = content
            .iter()
            .filter_map(ContentPart::text_value)
            .collect::<String>();
        Ok(AssistantMessage {
            content,
            text,
            usage: self.usage,
            usage_details: self.usage_details,
            structured_output: None,
            finish_reason,
            local_request_id: self.local_request_id,
            provider_request_id: self.provider_request_id,
            generation_id: self.generation_id,
            model: None,
        })
    }
}
