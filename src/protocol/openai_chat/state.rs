use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

use super::wire::{ChatCompletionChunkWire, ChoiceWire, DeltaWire, ToolCallDeltaWire, UsageWire};
use crate::domain::{
    AssistantEvent, ContentIndex, FinishReason, GenerationId, LocalRequestId, ModelRef,
    ProviderRequestId, ResourceLimits, TokenCount, ToolArguments, ToolCall, ToolCallId, ToolName,
    Usage, UsageDetails, UsageMergeOutcome, WireToolIndex, merge_usage_details,
};
use crate::error::{
    ErrorStage, LlmError, ProtocolError, TruncatedStreamError, UnknownFinishReason,
    UnsupportedResponseSemantics,
};
use crate::transport::{ByteStream, SseDecoder, SseEvent};

/// Stable request context supplied by the future client orchestration layer.
#[derive(Clone, Debug)]
pub(crate) struct OpenAiChatStreamContext {
    local_request_id: LocalRequestId,
    provider_request_id: Option<ProviderRequestId>,
    _model: ModelRef,
}

impl OpenAiChatStreamContext {
    pub(crate) fn new(
        local_request_id: LocalRequestId,
        provider_request_id: Option<ProviderRequestId>,
        model: ModelRef,
    ) -> Self {
        Self {
            local_request_id,
            provider_request_id,
            _model: model,
        }
    }
}

/// Converts an SDK byte stream into assistant events.
pub(crate) fn decode_openai_chat_stream(
    body: ByteStream,
    context: OpenAiChatStreamContext,
) -> OpenAiChatEventStream {
    decode_openai_chat_stream_with_limits(body, context, ResourceLimits::official())
}

/// Converts an SDK byte stream into assistant events using explicit resource limits.
pub(crate) fn decode_openai_chat_stream_with_limits(
    body: ByteStream,
    context: OpenAiChatStreamContext,
    limits: ResourceLimits,
) -> OpenAiChatEventStream {
    OpenAiChatEventStream {
        source: SseDecoder::new(body),
        machine: ChatStateMachine::new(context, limits),
        pending: VecDeque::new(),
        terminal: false,
    }
}

/// Stream adapter joining the protocol-neutral SSE decoder to Chat semantics.
pub(crate) struct OpenAiChatEventStream {
    source: SseDecoder,
    machine: ChatStateMachine,
    pending: VecDeque<Result<AssistantEvent, LlmError>>,
    terminal: bool,
}

impl fmt::Debug for OpenAiChatEventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiChatEventStream")
            .field("machine", &self.machine)
            .field("pending_events", &self.pending.len())
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl Stream for OpenAiChatEventStream {
    type Item = Result<AssistantEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        if let Some(item) = stream.pending.pop_front() {
            return Poll::Ready(Some(item));
        }
        if stream.terminal {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut stream.source).poll_next(context) {
                Poll::Ready(Some(Ok(event))) => match stream.machine.accept(&event) {
                    Ok(events) => {
                        stream.pending.extend(events.into_iter().map(Ok));
                        if let Some(item) = stream.pending.pop_front() {
                            return Poll::Ready(Some(item));
                        }
                    }
                    Err(error) => {
                        stream.terminal = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                },
                Poll::Ready(Some(Err(error))) => {
                    stream.terminal = true;
                    return Poll::Ready(Some(Err(error.into_llm_error())));
                }
                Poll::Ready(None) => {
                    stream.terminal = true;
                    match stream.machine.finish() {
                        Ok(events) => {
                            stream.pending.extend(events.into_iter().map(Ok));
                            return Poll::Ready(stream.pending.pop_front());
                        }
                        Err(error) => return Poll::Ready(Some(Err(error))),
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct ChatStateMachine {
    context: OpenAiChatStreamContext,
    limits: ResourceLimits,
    started: bool,
    text: ContentBlockState,
    refusal: ContentBlockState,
    tools: BTreeMap<WireToolIndex, PendingToolCall>,
    tool_order: Vec<WireToolIndex>,
    next_content_index: u32,
    total_tool_argument_bytes: usize,
    finish_reason: Option<FinishReason>,
    seen_done: bool,
    usage_details: Option<UsageDetails>,
    generation_id: Option<GenerationId>,
    response_model: Option<String>,
    unknown_fields: BTreeSet<String>,
}

impl fmt::Debug for ChatStateMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatStateMachine")
            .field("started", &self.started)
            .field("text", &self.text)
            .field("refusal", &self.refusal)
            .field("tool_calls", &self.tool_order.len())
            .field("finish_seen", &self.finish_reason.is_some())
            .field("done_seen", &self.seen_done)
            .field("usage_seen", &self.usage_details.is_some())
            .field("generation_id_seen", &self.generation_id.is_some())
            .field("response_model_seen", &self.response_model.is_some())
            .field("unknown_field_count", &self.unknown_fields.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentBlockState {
    NotStarted,
    Open { index: ContentIndex },
    Closed,
}

struct PendingToolCall {
    wire_index: WireToolIndex,
    domain_content_index: ContentIndex,
    provider_call_id: Option<ToolCallId>,
    name_buffer: String,
    arguments_buffer: String,
    start_emitted: bool,
    ended: bool,
}

impl fmt::Debug for PendingToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingToolCall")
            .field("wire_index", &self.wire_index.get())
            .field("domain_content_index", &self.domain_content_index.get())
            .field("has_id", &self.provider_call_id.is_some())
            .field("name_bytes", &self.name_buffer.len())
            .field("arguments_bytes", &self.arguments_buffer.len())
            .field("start_emitted", &self.start_emitted)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl ChatStateMachine {
    fn new(context: OpenAiChatStreamContext, limits: ResourceLimits) -> Self {
        Self {
            context,
            limits,
            started: false,
            text: ContentBlockState::NotStarted,
            refusal: ContentBlockState::NotStarted,
            tools: BTreeMap::new(),
            tool_order: Vec::new(),
            next_content_index: 0,
            total_tool_argument_bytes: 0,
            finish_reason: None,
            seen_done: false,
            usage_details: None,
            generation_id: None,
            response_model: None,
            unknown_fields: BTreeSet::new(),
        }
    }

    fn protocol(message: impl Into<String>) -> LlmError {
        ProtocolError::at_stage(ErrorStage::Protocol, message).into()
    }

    fn accept(&mut self, event: &SseEvent) -> Result<Vec<AssistantEvent>, LlmError> {
        if self.seen_done {
            if event.data() == "[DONE]" {
                return Err(Self::protocol("duplicate [DONE] marker"));
            }
            return Err(Self::protocol("data received after [DONE] marker"));
        }
        if event.data() == "[DONE]" {
            if self.finish_reason.is_none() {
                return Err(Self::protocol(
                    "[DONE] received without a supported finish reason",
                ));
            }
            if self.has_open_blocks() {
                return Err(Self::protocol(
                    "[DONE] received before all content blocks ended",
                ));
            }
            self.seen_done = true;
            return Ok(Vec::new());
        }

        let chunk: ChatCompletionChunkWire =
            serde_json::from_str(event.data()).map_err(|error| {
                let category = match error.classify() {
                    serde_json::error::Category::Io => "I/O",
                    serde_json::error::Category::Syntax => "syntax",
                    serde_json::error::Category::Data => "data",
                    serde_json::error::Category::Eof => "EOF",
                };
                LlmError::from(ProtocolError::at_stage(
                    ErrorStage::Json,
                    format!(
                        "JSON {category} error at line {} column {}",
                        error.line(),
                        error.column()
                    ),
                ))
            })?;
        if chunk.error.is_some() {
            return Err(Self::protocol("provider returned a JSON error object"));
        }
        if chunk
            .object
            .as_deref()
            .is_some_and(|object| object != "chat.completion.chunk")
        {
            return Err(Self::protocol("invalid chat completion chunk object"));
        }
        self.record_unknown_fields(&chunk);

        let prepared = PreparedChunk::validate(&chunk, self.finish_reason.is_some())?;
        self.observe_identity(chunk.id.as_deref(), chunk.model.as_deref())?;

        let mut events = Vec::new();
        self.emit_start(&mut events);

        if let Some(choice) = chunk.choices.first() {
            self.apply_choice(choice, prepared.finish_reason, &mut events)?;
        }
        if let Some(details) = prepared.usage {
            let (merged, outcome) = merge_usage_details(self.usage_details, details)
                .map_err(|error| Self::protocol(error.message()))?;
            match outcome {
                UsageMergeOutcome::Unchanged => {}
                UsageMergeOutcome::EmitP1 { details } => {
                    let usage = core_usage_from_details(details).map_err(Self::protocol)?;
                    events.push(AssistantEvent::Usage(usage));
                    events.push(AssistantEvent::DetailedUsage(details));
                }
                UsageMergeOutcome::EmitDetailed { details } => {
                    events.push(AssistantEvent::DetailedUsage(details));
                }
            }
            self.usage_details = Some(merged);
        }
        Ok(events)
    }

    fn record_unknown_fields(&mut self, chunk: &ChatCompletionChunkWire) {
        record_field_names(&mut self.unknown_fields, "chunk", chunk.extra.keys());
        for choice in &chunk.choices {
            record_field_names(&mut self.unknown_fields, "choice", choice.extra.keys());
            if let Some(delta) = &choice.delta {
                record_field_names(&mut self.unknown_fields, "delta", delta.extra.keys());
                if let Some(tool_calls) = &delta.tool_calls {
                    for tool_call in tool_calls {
                        record_field_names(
                            &mut self.unknown_fields,
                            "tool_call",
                            tool_call.extra.keys(),
                        );
                        if let Some(function) = &tool_call.function {
                            record_field_names(
                                &mut self.unknown_fields,
                                "function",
                                function.extra.keys(),
                            );
                        }
                    }
                }
            }
        }
        if let Some(usage) = &chunk.usage {
            record_field_names(&mut self.unknown_fields, "usage", usage.extra.keys());
        }
    }

    fn observe_identity(
        &mut self,
        generation_id: Option<&str>,
        response_model: Option<&str>,
    ) -> Result<(), LlmError> {
        if let Some(raw) = generation_id {
            let parsed = GenerationId::new(raw)
                .map_err(|_| Self::protocol("invalid generation id in response chunk"))?;
            if self
                .generation_id
                .as_ref()
                .is_some_and(|previous| previous != &parsed)
            {
                return Err(Self::protocol("generation id changed within stream"));
            }
            self.generation_id.get_or_insert(parsed);
        }
        if let Some(model) = response_model {
            if model.is_empty() || model.trim() != model {
                return Err(Self::protocol("invalid model identifier in response chunk"));
            }
            if self
                .response_model
                .as_deref()
                .is_some_and(|previous| previous != model)
            {
                return Err(Self::protocol("response model changed within stream"));
            }
            if self.response_model.is_none() {
                self.response_model = Some(model.to_owned());
            }
        }
        Ok(())
    }

    fn emit_start(&mut self, events: &mut Vec<AssistantEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        events.push(AssistantEvent::Start {
            local_request_id: self.context.local_request_id.clone(),
            provider_request_id: self.context.provider_request_id.clone(),
            generation_id: self.generation_id.clone(),
        });
    }

    fn apply_choice(
        &mut self,
        choice: &ChoiceWire,
        finish_reason: Option<FinishReason>,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if let Some(delta) = &choice.delta {
            self.apply_delta(delta, events)?;
        }
        if let Some(reason) = finish_reason {
            self.apply_finish(reason, events)?;
        }
        Ok(())
    }

    fn apply_delta(
        &mut self,
        delta: &DeltaWire,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if let Some(content) = delta.content.as_deref()
            && !content.is_empty()
        {
            self.push_text(content, events)?;
        }
        if let Some(refusal) = delta.refusal.as_deref()
            && !refusal.is_empty()
        {
            self.push_refusal(refusal, events)?;
        }
        if let Some(tool_calls) = &delta.tool_calls {
            for tool_call in tool_calls {
                self.push_tool_delta(tool_call, events)?;
            }
        }
        Ok(())
    }

    fn push_text(
        &mut self,
        content: &str,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if !matches!(self.refusal, ContentBlockState::NotStarted) {
            return Err(UnsupportedResponseSemantics::new("text with refusal").into());
        }
        if self.text == ContentBlockState::Closed {
            return Err(Self::protocol("text delta received after finish reason"));
        }
        if self.text == ContentBlockState::NotStarted {
            let index = self.allocate_content_index()?;
            self.text = ContentBlockState::Open { index };
            events.push(AssistantEvent::TextStart { index });
        }
        let ContentBlockState::Open { index } = self.text else {
            return Err(Self::protocol("text block is not open"));
        };
        events.push(AssistantEvent::TextDelta {
            index,
            delta: content.to_owned(),
        });
        Ok(())
    }

    fn push_refusal(
        &mut self,
        content: &str,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if !matches!(self.text, ContentBlockState::NotStarted) {
            return Err(UnsupportedResponseSemantics::new("refusal with text").into());
        }
        if !self.tools.is_empty() {
            return Err(UnsupportedResponseSemantics::new("refusal with tool_calls").into());
        }
        if self.refusal == ContentBlockState::Closed {
            return Err(Self::protocol("refusal delta received after finish reason"));
        }
        if self.refusal == ContentBlockState::NotStarted {
            let index = self.allocate_content_index()?;
            self.refusal = ContentBlockState::Open { index };
            events.push(AssistantEvent::RefusalStart { index });
        }
        let ContentBlockState::Open { index } = self.refusal else {
            return Err(Self::protocol("refusal block is not open"));
        };
        events.push(AssistantEvent::RefusalDelta {
            index,
            delta: content.to_owned(),
        });
        Ok(())
    }

    fn push_tool_delta(
        &mut self,
        delta: &ToolCallDeltaWire,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if !matches!(self.refusal, ContentBlockState::NotStarted) {
            return Err(UnsupportedResponseSemantics::new("tool_calls with refusal").into());
        }
        if self.finish_reason.is_some() {
            return Err(Self::protocol(
                "tool call delta received after finish reason",
            ));
        }
        if let Some(kind) = delta.kind.as_deref()
            && kind != "function"
        {
            return Err(UnsupportedResponseSemantics::new("tool_call.type").into());
        }

        let wire_index = parse_wire_tool_index(delta.index)?;
        self.ensure_tool_accumulator(wire_index, delta.id.as_deref(), events)?;
        self.append_tool_fragments(wire_index, delta, events)
    }

    fn ensure_tool_accumulator(
        &mut self,
        wire_index: WireToolIndex,
        raw_id: Option<&str>,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if let Some(pending) = self.tools.get_mut(&wire_index) {
            if pending.ended {
                return Err(Self::protocol(
                    "tool call delta received after tool call end",
                ));
            }
            if let Some(raw_id) = raw_id {
                let parsed = parse_tool_call_id(raw_id)?;
                match &pending.provider_call_id {
                    Some(existing) if existing != &parsed => {
                        return Err(Self::protocol("conflicting tool call id for wire index"));
                    }
                    Some(_) => {}
                    None => pending.provider_call_id = Some(parsed),
                }
            }
            return Ok(());
        }

        if self.tool_order.len() >= self.limits.max_tool_calls {
            return Err(Self::protocol("tool call count exceeds resource limit"));
        }
        let domain_content_index = self.allocate_content_index()?;
        let provider_call_id = raw_id.map(parse_tool_call_id).transpose()?;
        self.tool_order.push(wire_index);
        self.tools.insert(
            wire_index,
            PendingToolCall {
                wire_index,
                domain_content_index,
                provider_call_id: provider_call_id.clone(),
                name_buffer: String::new(),
                arguments_buffer: String::new(),
                start_emitted: true,
                ended: false,
            },
        );
        events.push(AssistantEvent::ToolCallStart {
            index: domain_content_index,
            wire_index,
            id: provider_call_id,
        });
        Ok(())
    }

    fn append_tool_fragments(
        &mut self,
        wire_index: WireToolIndex,
        delta: &ToolCallDeltaWire,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        let pending = self
            .tools
            .get_mut(&wire_index)
            .ok_or_else(|| Self::protocol("missing tool call accumulator"))?;
        if pending.ended {
            return Err(Self::protocol(
                "tool call delta received after tool call end",
            ));
        }

        let name_delta = delta
            .function
            .as_ref()
            .and_then(|function| function.name.as_ref())
            .filter(|name| !name.is_empty())
            .cloned();
        let arguments_delta = delta
            .function
            .as_ref()
            .and_then(|function| function.arguments.as_ref())
            .filter(|arguments| !arguments.is_empty())
            .cloned();

        if let Some(delta) = &name_delta {
            if pending.name_buffer.len().saturating_add(delta.len()) > ToolName::MAX_BYTES {
                return Err(Self::protocol("tool name exceeds resource limit"));
            }
            pending.name_buffer.push_str(delta);
        }
        if let Some(delta) = &arguments_delta {
            let next_call = pending.arguments_buffer.len().saturating_add(delta.len());
            if next_call > self.limits.max_tool_arguments_bytes {
                return Err(Self::protocol(
                    "tool arguments exceed per-call resource limit",
                ));
            }
            let next_total = self.total_tool_argument_bytes.saturating_add(delta.len());
            if next_total > self.limits.max_all_tool_arguments_bytes {
                return Err(Self::protocol(
                    "tool arguments exceed aggregate resource limit",
                ));
            }
            pending.arguments_buffer.push_str(delta);
            self.total_tool_argument_bytes = next_total;
        }

        if name_delta.is_some() || arguments_delta.is_some() {
            events.push(AssistantEvent::ToolCallDelta {
                index: pending.domain_content_index,
                wire_index,
                name_delta,
                arguments_delta,
            });
        }
        Ok(())
    }

    fn apply_finish(
        &mut self,
        reason: FinishReason,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        if self.finish_reason.is_some() {
            return Err(Self::protocol("duplicate finish reason"));
        }

        match reason {
            FinishReason::ToolCalls => {
                self.close_text_if_open(events);
                if !matches!(self.refusal, ContentBlockState::NotStarted) {
                    return Err(UnsupportedResponseSemantics::new("tool_calls with refusal").into());
                }
                if self.tool_order.is_empty() {
                    return Err(Self::protocol(
                        "finish reason tool_calls without tool call deltas",
                    ));
                }
                let order = self.tool_order.clone();
                for wire_index in order {
                    self.end_tool_call(wire_index, events)?;
                }
            }
            FinishReason::Stop | FinishReason::Length => {
                if !self.tools.is_empty() {
                    return Err(Self::protocol(
                        "non-tool finish reason received with tool call deltas",
                    ));
                }
                if matches!(self.refusal, ContentBlockState::Open { .. }) {
                    self.close_refusal(events);
                } else if matches!(self.text, ContentBlockState::Open { .. }) {
                    self.close_text_if_open(events);
                } else if matches!(self.text, ContentBlockState::NotStarted)
                    && matches!(self.refusal, ContentBlockState::NotStarted)
                {
                    // Preserve P1 empty-completion boundaries for text-only finishes.
                    let index = self.allocate_content_index()?;
                    self.text = ContentBlockState::Open { index };
                    events.push(AssistantEvent::TextStart { index });
                    self.close_text_if_open(events);
                }
            }
            FinishReason::ContentFilter => {
                if !self.tools.is_empty() {
                    return Err(Self::protocol(
                        "content_filter finish reason received with tool call deltas",
                    ));
                }
                // Official OpenAI Profile maps content_filter to a typed protocol error.
                // The finish reason remains available on the error path, but no success Done.
                self.finish_reason = Some(reason.clone());
                return Err(Self::protocol(
                    "content_filter finish reason is not a successful completion",
                ));
            }
            FinishReason::Unknown(raw) => {
                return Err(UnknownFinishReason::new(bounded_label(&raw, 64)).into());
            }
        }

        self.finish_reason = Some(reason);
        Ok(())
    }

    fn close_text_if_open(&mut self, events: &mut Vec<AssistantEvent>) {
        if let ContentBlockState::Open { index } = self.text {
            self.text = ContentBlockState::Closed;
            events.push(AssistantEvent::TextEnd { index });
        }
    }

    fn close_refusal(&mut self, events: &mut Vec<AssistantEvent>) {
        if let ContentBlockState::Open { index } = self.refusal {
            self.refusal = ContentBlockState::Closed;
            events.push(AssistantEvent::RefusalEnd { index });
        }
    }

    fn end_tool_call(
        &mut self,
        wire_index: WireToolIndex,
        events: &mut Vec<AssistantEvent>,
    ) -> Result<(), LlmError> {
        let pending = self
            .tools
            .get_mut(&wire_index)
            .ok_or_else(|| Self::protocol("missing tool call accumulator"))?;
        if pending.ended {
            return Err(Self::protocol("duplicate tool call end"));
        }
        let id = pending
            .provider_call_id
            .clone()
            .ok_or_else(|| Self::protocol("tool call completed without an id"))?;
        if pending.name_buffer.is_empty() {
            return Err(Self::protocol("tool call completed without a name"));
        }
        let name = ToolName::new(pending.name_buffer.clone())
            .map_err(|_| Self::protocol("tool call completed with an invalid name"))?;
        let arguments = ToolArguments::from_raw_json(pending.arguments_buffer.clone())
            .map_err(|_| Self::protocol("tool call completed with incomplete JSON arguments"))?;
        if json_depth(arguments.value()) > self.limits.max_schema_depth {
            return Err(Self::protocol(
                "tool call arguments exceed maximum JSON nesting depth",
            ));
        }
        let call = ToolCall::new(id, name, arguments);
        let index = pending.domain_content_index;
        pending.ended = true;
        events.push(AssistantEvent::ToolCallEnd { index, call });
        Ok(())
    }

    fn allocate_content_index(&mut self) -> Result<ContentIndex, LlmError> {
        let index = ContentIndex::new(self.next_content_index);
        self.next_content_index = self
            .next_content_index
            .checked_add(1)
            .ok_or_else(|| Self::protocol("content index overflow"))?;
        Ok(index)
    }

    fn has_open_blocks(&self) -> bool {
        matches!(self.text, ContentBlockState::Open { .. })
            || matches!(self.refusal, ContentBlockState::Open { .. })
            || self.tools.values().any(|tool| !tool.ended)
    }

    fn finish(&mut self) -> Result<Vec<AssistantEvent>, LlmError> {
        if !self.seen_done {
            return Err(TruncatedStreamError.into());
        }
        if self.has_open_blocks() {
            return Err(Self::protocol(
                "stream ended with incomplete content blocks",
            ));
        }
        let Some(finish_reason) = self.finish_reason.clone() else {
            return Err(Self::protocol(
                "stream ended without a supported finish reason",
            ));
        };
        Ok(vec![AssistantEvent::Done { finish_reason }])
    }
}

struct PreparedChunk {
    finish_reason: Option<FinishReason>,
    usage: Option<UsageDetails>,
}

impl PreparedChunk {
    fn validate(
        chunk: &ChatCompletionChunkWire,
        finish_already_seen: bool,
    ) -> Result<Self, LlmError> {
        if chunk.choices.len() > 1 {
            return Err(UnsupportedResponseSemantics::new("multiple choices").into());
        }
        if chunk.choices.is_empty() && chunk.usage.is_none() {
            return Err(ChatStateMachine::protocol(
                "chunk has neither a choice nor usage",
            ));
        }

        let finish_reason = if let Some(choice) = chunk.choices.first() {
            Self::validate_choice(choice, finish_already_seen)?
        } else {
            None
        };
        let usage = chunk.usage.as_ref().map(parse_usage_details).transpose()?;
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
                return Err(ChatStateMachine::protocol("duplicate finish reason"));
            }
            return Err(ChatStateMachine::protocol(
                "choice data received after finish reason",
            ));
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

fn parse_usage_details(wire: &UsageWire) -> Result<UsageDetails, LlmError> {
    let input = optional_token_count(wire.prompt_tokens, "usage.prompt_tokens")?;
    let output = optional_token_count(wire.completion_tokens, "usage.completion_tokens")?;
    let total = optional_token_count(wire.total_tokens, "usage.total_tokens")?;
    let cached_input = optional_token_count(
        wire.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        "usage.prompt_tokens_details.cached_tokens",
    )?;
    let cache_write = optional_token_count(
        wire.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens),
        "usage.prompt_tokens_details.cache_write_tokens",
    )?;
    let reasoning = optional_token_count(
        wire.completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
        "usage.completion_tokens_details.reasoning_tokens",
    )?;

    let details = UsageDetails::new(input, output, total, cached_input, cache_write, reasoning);
    details
        .validate_relationships()
        .map_err(|error| ChatStateMachine::protocol(error.message()))?;
    if !details.has_any_known() {
        return Err(ChatStateMachine::protocol(
            "usage object did not report any token counts",
        ));
    }
    Ok(details)
}

fn optional_token_count(value: Option<i64>, field: &str) -> Result<TokenCount, LlmError> {
    match value {
        None => Ok(TokenCount::Unknown),
        Some(raw) => {
            let count = u64::try_from(raw)
                .map_err(|_| ChatStateMachine::protocol(format!("{field} must be non-negative")))?;
            Ok(TokenCount::Known(count))
        }
    }
}

fn core_usage_from_details(details: UsageDetails) -> Result<Usage, &'static str> {
    match (
        details.input_tokens(),
        details.output_tokens(),
        details.total_tokens(),
    ) {
        (TokenCount::Known(input), TokenCount::Known(output), TokenCount::Known(total)) => {
            Usage::new(input, output, total)
                .map_err(|_| "usage total does not equal input + output")
        }
        _ => Err("core usage counters are incomplete"),
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

fn parse_wire_tool_index(raw: i64) -> Result<WireToolIndex, LlmError> {
    if raw < 0 {
        return Err(ChatStateMachine::protocol(
            "tool call index must be non-negative",
        ));
    }
    let value = u32::try_from(raw)
        .map_err(|_| ChatStateMachine::protocol("tool call index exceeds u32 range"))?;
    Ok(WireToolIndex::new(value))
}

fn parse_tool_call_id(raw: &str) -> Result<ToolCallId, LlmError> {
    ToolCallId::new(raw).map_err(|_| ChatStateMachine::protocol("invalid tool call id"))
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn bounded_label(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn record_field_names<'a>(
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{StreamExt as _, stream};

    use super::*;

    fn context() -> OpenAiChatStreamContext {
        OpenAiChatStreamContext::new(
            LocalRequestId::new("local-state-test").unwrap(),
            Some(ProviderRequestId::new("header-request-id").unwrap()),
            ModelRef::new("openai", "gpt-test").unwrap(),
        )
    }

    async fn decode(input: &'static [u8]) -> Vec<Result<AssistantEvent, LlmError>> {
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(input))]));
        decode_openai_chat_stream(body, context()).collect().await
    }

    async fn decode_owned(
        chunks: Vec<Result<Bytes, LlmError>>,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        let body: ByteStream = Box::pin(stream::iter(chunks));
        decode_openai_chat_stream(body, context()).collect().await
    }

    fn sse_chunks(lines: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            out.extend_from_slice(b"data: ");
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\n\n");
        }
        out
    }

    #[tokio::test]
    async fn text_fixture_produces_exact_event_sequence() {
        let events = decode(include_bytes!(
            "../../../tests/fixtures/responses/openai_chat/text.sse"
        ))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        assert_eq!(events.len(), 8);
        assert!(matches!(
            &events[0],
            AssistantEvent::Start {
                provider_request_id: Some(id),
                generation_id: Some(generation),
                ..
            } if id.as_str() == "header-request-id" && generation.as_str() == "chatcmpl-text"
        ));
        assert_eq!(
            events[1],
            AssistantEvent::TextStart {
                index: ContentIndex::new(0)
            }
        );
        assert_eq!(
            events[2],
            AssistantEvent::TextDelta {
                index: ContentIndex::new(0),
                delta: "Hel".to_owned()
            }
        );
        assert_eq!(
            events[3],
            AssistantEvent::TextDelta {
                index: ContentIndex::new(0),
                delta: "lo".to_owned()
            }
        );
        assert_eq!(
            events[4],
            AssistantEvent::TextEnd {
                index: ContentIndex::new(0)
            }
        );
        assert_eq!(
            events[5],
            AssistantEvent::Usage(Usage::new(2, 1, 3).unwrap())
        );
        assert!(matches!(events[6], AssistantEvent::DetailedUsage(_)));
        assert_eq!(
            events[7],
            AssistantEvent::Done {
                finish_reason: FinishReason::Stop
            }
        );
    }

    #[tokio::test]
    async fn successful_fixture_variants_preserve_boundaries_and_usage() {
        for fixture in [
            include_bytes!("../../../tests/fixtures/responses/openai_chat/usage-only.sse")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/responses/openai_chat/empty-content.sse")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/responses/openai_chat/unknown-fields.sse")
                .as_slice(),
        ] {
            let events = decode(fixture)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(matches!(events.first(), Some(AssistantEvent::Start { .. })));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AssistantEvent::TextStart { .. }))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AssistantEvent::TextEnd { .. }))
                    .count(),
                1
            );
            assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
        }
    }

    #[tokio::test]
    async fn single_tool_call_stream_emits_start_delta_end_and_tool_calls_finish() {
        let fixture =
            include_bytes!("../../../tests/fixtures/phase-2/streams/tool-calls/single-call.sse");
        let events = decode(fixture)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(events[0], AssistantEvent::Start { .. }));
        assert!(matches!(
            &events[1],
            AssistantEvent::ToolCallStart {
                index,
                wire_index,
                id: Some(id),
            } if index.get() == 0
                && wire_index.get() == 0
                && id.as_str() == "call_weather"
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::ToolCallDelta {
                name_delta: Some(name),
                ..
            } if name == "get_weather"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AssistantEvent::ToolCallEnd {
                call,
                ..
            } if call.id().as_str() == "call_weather"
                && call.name().as_str() == "get_weather"
                && call.arguments().raw_json() == r#"{"city":"Paris"}"#
        )));
        assert_eq!(
            events.last(),
            Some(&AssistantEvent::Done {
                finish_reason: FinishReason::ToolCalls
            })
        );
    }

    #[tokio::test]
    async fn parallel_tool_calls_preserve_first_seen_domain_order() {
        let fixture = include_bytes!(
            "../../../tests/fixtures/phase-2/streams/tool-calls/parallel-interleaved.sse"
        );
        let events = decode(fixture)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let ends: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::ToolCallEnd { index, call } => {
                    Some((index.get(), call.name().as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(ends, vec![(0, "alpha"), (1, "beta")]);
        assert!(matches!(
            events.last(),
            Some(AssistantEvent::Done {
                finish_reason: FinishReason::ToolCalls
            })
        ));
    }

    #[tokio::test]
    async fn name_and_argument_splits_reassemble_identically() {
        for fixture in [
            include_bytes!("../../../tests/fixtures/phase-2/streams/tool-calls/name-split.sse")
                .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/phase-2/streams/tool-calls/arguments-char-split.sse"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/phase-2/streams/tool-calls/id-first-chunk-only.sse"
            )
            .as_slice(),
            include_bytes!(
                "../../../tests/fixtures/phase-2/streams/tool-calls/usage-after-tool.sse"
            )
            .as_slice(),
        ] {
            let events = decode(fixture)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AssistantEvent::ToolCallStart { .. }))
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AssistantEvent::ToolCallEnd { .. }))
                    .count(),
                1
            );
            assert!(matches!(
                events.last(),
                Some(AssistantEvent::Done {
                    finish_reason: FinishReason::ToolCalls
                })
            ));
        }
    }

    #[tokio::test]
    async fn tool_stream_failure_matrix_is_typed_and_terminal() {
        type ErrorCase = (&'static [u8], fn(&LlmError) -> bool);
        let cases: &[ErrorCase] = &[
            (
                include_bytes!(
                    "../../../tests/fixtures/phase-2/streams/tool-calls/incomplete-arguments.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/phase-2/streams/tool-calls/conflicting-id.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/phase-2/streams/tool-calls/duplicate-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/phase-2/streams/tool-calls/done-before-call-end.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/tool-finish.sse"),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
        ];

        for (fixture, classify) in cases {
            let results = decode(fixture).await;
            let errors: Vec<_> = results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .collect();
            assert_eq!(errors.len(), 1, "fixture should produce one error");
            assert!(classify(errors[0]), "unexpected error: {:?}", errors[0]);
            assert!(
                !results
                    .iter()
                    .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
            );
        }
    }

    #[tokio::test]
    async fn oversized_tool_arguments_fail_with_reduced_limits() {
        let fixture = include_bytes!(
            "../../../tests/fixtures/phase-2/streams/tool-calls/oversized-arguments.sse"
        );
        let mut limits = ResourceLimits::official();
        limits.max_tool_arguments_bytes = 16;
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(fixture))]));
        let results: Vec<_> = decode_openai_chat_stream_with_limits(body, context(), limits)
            .collect()
            .await;
        let errors: Vec<_> = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], LlmError::Protocol(_)));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::ToolCallEnd { .. })))
        );
    }

    #[tokio::test]
    async fn random_and_forced_partitions_match_single_chunk_baseline() {
        let fixtures: &[&[u8]] = &[
            include_bytes!("../../../tests/fixtures/phase-2/streams/tool-calls/single-call.sse"),
            include_bytes!(
                "../../../tests/fixtures/phase-2/streams/tool-calls/parallel-interleaved.sse"
            ),
            include_bytes!(
                "../../../tests/fixtures/phase-2/streams/tool-calls/arguments-char-split.sse"
            ),
            include_bytes!("../../../tests/fixtures/responses/openai_chat/text.sse"),
        ];

        for fixture in fixtures {
            let baseline = decode(fixture)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            let bytewise = fixture
                .iter()
                .map(|byte| Ok(Bytes::copy_from_slice(std::slice::from_ref(byte))))
                .collect();
            let partitioned = decode_owned(bytewise)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(partitioned, baseline);

            // Force cuts inside likely sensitive regions of the fixture payload.
            for cut in [1_usize, 7, 15, 31, fixture.len() / 2] {
                if cut == 0 || cut >= fixture.len() {
                    continue;
                }
                let chunks = vec![
                    Ok(Bytes::copy_from_slice(&fixture[..cut])),
                    Ok(Bytes::copy_from_slice(&fixture[cut..])),
                ];
                let split = decode_owned(chunks)
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(split, baseline, "cut at {cut}");
            }
        }
    }

    #[tokio::test]
    async fn fixture_failure_matrix_is_typed_and_terminal() {
        type ErrorCase = (&'static [u8], fn(&LlmError) -> bool);
        let cases: &[ErrorCase] = &[
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/unknown-finish-reason.sse"
                ),
                |error| matches!(error, LlmError::UnknownFinishReason(_)),
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/content-filter.sse"),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/nonzero-choice-index.sse"
                ),
                |error| matches!(error, LlmError::UnsupportedResponseSemantics(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/done-without-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/finish-without-done.sse"
                ),
                |error| matches!(error, LlmError::TruncatedStream(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/duplicate-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/duplicate-done.sse"),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/data-after-done.sse"),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/responses/openai_chat/json-error-object.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/malformed-json.sse"),
                |error| {
                    matches!(
                        error,
                        LlmError::Protocol(inner) if inner.stage() == ErrorStage::Json
                    )
                },
            ),
            (
                include_bytes!("../../../tests/fixtures/responses/openai_chat/truncated.sse"),
                |error| matches!(error, LlmError::TruncatedStream(_)),
            ),
        ];

        for (fixture, classify) in cases {
            let results = decode(fixture).await;
            let errors: Vec<_> = results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .collect();
            assert_eq!(errors.len(), 1);
            assert!(classify(errors[0]));
            assert!(
                !results
                    .iter()
                    .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
            );
        }
    }

    #[tokio::test]
    async fn malformed_json_diagnostics_do_not_echo_data() {
        let results = decode(include_bytes!(
            "../../../tests/fixtures/responses/openai_chat/malformed-json.sse"
        ))
        .await;
        let error = results.last().unwrap().as_ref().unwrap_err();
        assert!(!error.to_string().contains("canary-private-output"));
        assert!(!format!("{error:?}").contains("canary-private-output"));
    }

    #[tokio::test]
    async fn byte_by_byte_chat_fixture_matches_single_chunk() {
        let fixture = include_bytes!("../../../tests/fixtures/responses/openai_chat/text.sse");
        let baseline = decode(fixture)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let chunks = fixture
            .iter()
            .map(|byte| Ok(Bytes::copy_from_slice(std::slice::from_ref(byte))))
            .collect();
        let bytewise = decode_owned(chunks)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(bytewise, baseline);
    }

    #[tokio::test]
    async fn upstream_error_after_partial_text_is_the_only_terminal_result() {
        let first = Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-partial\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
        );
        let results = decode_owned(vec![Ok(first), Err(LlmError::Cancelled)]).await;
        assert!(results.iter().any(|event| matches!(
            event,
            Ok(AssistantEvent::TextDelta { delta, .. }) if delta == "partial"
        )));
        assert_eq!(
            results
                .iter()
                .filter(|event| matches!(event, Err(LlmError::Cancelled)))
                .count(),
            1
        );
        assert!(
            !results
                .iter()
                .any(|event| matches!(event, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn changing_identity_and_conflicting_usage_fail_closed() {
        let cases = [
            concat!(
                "data: {\"id\":\"one\",\"model\":\"gpt-a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"two\",\"model\":\"gpt-a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"b\"},\"finish_reason\":null}]}\n\n",
            ),
            concat!(
                "data: {\"id\":\"one\",\"model\":\"gpt-a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"one\",\"model\":\"gpt-b\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"b\"},\"finish_reason\":null}]}\n\n",
            ),
            concat!(
                "data: {\"id\":\"one\",\"model\":\"gpt-a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"id\":\"one\",\"model\":\"gpt-a\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
                "data: {\"id\":\"one\",\"model\":\"gpt-a\",\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}\n\n",
                "data: [DONE]\n\n",
            ),
        ];
        for input in cases {
            let results = decode_owned(vec![Ok(Bytes::copy_from_slice(input.as_bytes()))]).await;
            assert_eq!(
                results
                    .iter()
                    .filter(|result| matches!(result, Err(LlmError::Protocol(_))))
                    .count(),
                1
            );
            assert!(
                !results
                    .iter()
                    .any(|result| matches!(result, Ok(AssistantEvent::Done { .. })))
            );
        }
    }

    #[tokio::test]
    async fn identical_usage_is_idempotent_and_negative_usage_is_rejected() {
        let repeated = concat!(
            "data: {\"id\":\"usage\",\"model\":\"gpt-a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"usage\",\"model\":\"gpt-a\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            "data: {\"id\":\"usage\",\"model\":\"gpt-a\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        let events = decode_owned(vec![Ok(Bytes::copy_from_slice(repeated.as_bytes()))])
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantEvent::Usage(_)))
                .count(),
            1
        );
        assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));

        let negative = b"data: {\"id\":\"usage\",\"model\":\"gpt-a\",\"choices\":[],\"usage\":{\"prompt_tokens\":-1,\"completion_tokens\":1,\"total_tokens\":0}}\n\n";
        let results = decode_owned(vec![Ok(Bytes::from_static(negative))]).await;
        assert!(matches!(results.as_slice(), [Err(LlmError::Protocol(_))]));
    }

    #[tokio::test]
    async fn tool_arguments_and_accumulator_debug_do_not_expose_raw_payloads() {
        let arguments =
            ToolArguments::from_raw_json(r#"{"city":"Paris","secret":"argument-canary"}"#).unwrap();
        let arguments_debug = format!("{arguments:?}");
        assert!(!arguments_debug.contains("Paris"));
        assert!(!arguments_debug.contains("argument-canary"));
        assert!(arguments_debug.contains("raw_json_bytes"));

        let mut pending = PendingToolCall {
            wire_index: WireToolIndex::new(0),
            domain_content_index: ContentIndex::new(0),
            provider_call_id: Some(ToolCallId::new("call_secret").unwrap()),
            name_buffer: "get_weather".to_owned(),
            arguments_buffer: r#"{"secret":"argument-canary"}"#.to_owned(),
            start_emitted: true,
            ended: false,
        };
        let pending_debug = format!("{pending:?}");
        assert!(!pending_debug.contains("argument-canary"));
        assert!(!pending_debug.contains("get_weather"));
        assert!(pending_debug.contains("arguments_bytes"));

        pending.ended = true;
        let machine_debug = format!(
            "{:?}",
            ChatStateMachine {
                context: context(),
                limits: ResourceLimits::official(),
                started: true,
                text: ContentBlockState::NotStarted,
                refusal: ContentBlockState::NotStarted,
                tools: BTreeMap::from([(WireToolIndex::new(0), pending)]),
                tool_order: vec![WireToolIndex::new(0)],
                next_content_index: 1,
                total_tool_argument_bytes: 32,
                finish_reason: Some(FinishReason::ToolCalls),
                seen_done: true,
                usage_details: None,
                generation_id: None,
                response_model: None,
                unknown_fields: BTreeSet::new(),
            }
        );
        assert!(!machine_debug.contains("argument-canary"));
        assert!(machine_debug.contains("tool_calls: 1"));
    }

    #[test]
    fn unknown_field_audit_records_names_without_values() {
        let chunk: ChatCompletionChunkWire = serde_json::from_str(
            r#"{"id":"audit","model":"gpt","top_future":"canary-value","choices":[{"index":0,"choice_future":1,"delta":{"delta_future":true},"finish_reason":null}],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0,"usage_future":"private"}}"#,
        )
        .unwrap();
        let mut machine = ChatStateMachine::new(context(), ResourceLimits::official());
        machine.record_unknown_fields(&chunk);
        assert_eq!(
            machine.unknown_fields,
            BTreeSet::from([
                "choice.choice_future".to_owned(),
                "chunk.top_future".to_owned(),
                "delta.delta_future".to_owned(),
                "usage.usage_future".to_owned(),
            ])
        );
        let debug = format!("{machine:?}");
        assert!(debug.contains("unknown_field_count: 4"));
        assert!(!debug.contains("canary-value"));
        assert!(!debug.contains("private"));
    }

    #[test]
    fn sse_chunk_helper_is_available_for_local_construction() {
        let encoded = sse_chunks(&[r#"{"ok":true}"#, "[DONE]"]);
        assert!(encoded.starts_with(b"data: "));
        assert!(encoded.windows(6).any(|window| window == b"[DONE]"));
    }
}
