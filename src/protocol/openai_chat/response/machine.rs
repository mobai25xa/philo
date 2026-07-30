use std::collections::BTreeSet;
use std::fmt;

use super::super::wire::{ChatCompletionChunkWire, ChoiceWire, DeltaWire, ToolCallDeltaWire};
use super::stream::OpenAiChatStreamContext;
use super::terminal::{PreparedChunk, bounded_label, record_field_names};
use super::tool_calls::ToolCallAccumulator;
use crate::domain::{
    AssistantEvent, ContentIndex, FinishReason, GenerationId, ResponseFormat, SchemaLimits,
    UsageDetails, UsageMergeOutcome, merge_usage_details,
};
use crate::error::{
    ErrorStage, LlmError, ProtocolError, TruncatedStreamError, UnknownFinishReason,
    UnsupportedResponseSemantics,
};
use crate::plan::ResponseLimits;
use crate::protocol::structured_terminal::StructuredTerminal;
use crate::provider::ResponseCompat;
use crate::transport::SseEvent;

pub(crate) struct ChatStateMachine {
    context: OpenAiChatStreamContext,
    limits: ResponseLimits,
    started: bool,
    text: ContentBlockState,
    refusal: ContentBlockState,
    tools: ToolCallAccumulator,
    next_content_index: u32,
    finish_reason: Option<FinishReason>,
    duplicate_finish_seen: bool,
    seen_done: bool,
    terminal: StructuredTerminal,
    usage_details: Option<UsageDetails>,
    generation_id: Option<GenerationId>,
    response_model: Option<String>,
    unknown_fields: BTreeSet<String>,
    response_compat: ResponseCompat,
}

impl fmt::Debug for ChatStateMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatStateMachine")
            .field("started", &self.started)
            .field("text", &self.text)
            .field("refusal", &self.refusal)
            .field("tool_calls", &self.tools.len())
            .field("finish_seen", &self.finish_reason.is_some())
            .field("duplicate_finish_seen", &self.duplicate_finish_seen)
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

impl ChatStateMachine {
    pub(super) fn new_with_format(
        context: OpenAiChatStreamContext,
        response_format: ResponseFormat,
        limits: ResponseLimits,
        response_compat: ResponseCompat,
    ) -> Self {
        Self {
            context,
            limits,
            started: false,
            text: ContentBlockState::NotStarted,
            refusal: ContentBlockState::NotStarted,
            tools: ToolCallAccumulator::new(limits),
            next_content_index: 0,
            finish_reason: None,
            duplicate_finish_seen: false,
            seen_done: false,
            terminal: StructuredTerminal::new(
                response_format,
                SchemaLimits {
                    max_schema_bytes: usize::MAX,
                    max_schema_depth: limits.max_schema_depth,
                    max_json_array_items: limits.max_json_array_items,
                },
                limits.max_structured_output_bytes,
            ),
            usage_details: None,
            generation_id: None,
            response_model: None,
            unknown_fields: BTreeSet::new(),
            response_compat,
        }
    }

    fn protocol(message: impl Into<String>) -> LlmError {
        ProtocolError::at_stage(ErrorStage::Protocol, message).into()
    }

    pub(super) fn accept(&mut self, event: &SseEvent) -> Result<Vec<AssistantEvent>, LlmError> {
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
            self.terminal.validate_before_done(
                self.finish_reason
                    .as_ref()
                    .expect("finish reason checked above"),
                !self.tools.is_empty(),
                !matches!(self.refusal, ContentBlockState::NotStarted),
            )?;
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
        super::super::compat::error::validate_inline_error(
            chunk.error.is_some(),
            self.response_compat.inline_error,
        )?;
        if chunk
            .object
            .as_deref()
            .is_some_and(|object| object != "chat.completion.chunk")
        {
            return Err(Self::protocol("invalid chat completion chunk object"));
        }
        let prepared = PreparedChunk::validate(
            &chunk,
            self.finish_reason.as_ref(),
            self.duplicate_finish_seen,
            self.response_compat.finish_reason,
            self.response_compat.usage,
        )?;
        if chunk.choices.iter().any(|choice| {
            choice
                .delta
                .as_ref()
                .and_then(|delta| delta.tool_calls.as_ref())
                .is_some_and(|tool_calls| tool_calls.len() > self.limits.max_tool_calls)
        }) {
            return Err(Self::protocol("tool call array exceeds resource limit"));
        }
        self.record_unknown_fields(&chunk);
        self.observe_identity(chunk.id.as_deref(), chunk.model.as_deref())?;

        let mut events = Vec::new();
        self.emit_start(&mut events);

        if let Some(choice) = chunk.choices.first() {
            self.apply_choice(choice, prepared.finish_reason, &mut events)?;
        }
        if prepared.duplicate_finish {
            self.duplicate_finish_seen = true;
        }
        if let Some(details) = prepared.usage {
            let (merged, outcome) = merge_usage_details(self.usage_details, details)
                .map_err(|error| Self::protocol(error.message()))?;
            match outcome {
                UsageMergeOutcome::Unchanged => {}
                UsageMergeOutcome::EmitP1 { details } => {
                    let usage =
                        super::usage::core_usage_from_details(details).map_err(Self::protocol)?;
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
            if let Some(details) = &usage.prompt_tokens_details {
                record_field_names(
                    &mut self.unknown_fields,
                    "usage.prompt_tokens_details",
                    details.extra.keys(),
                );
            }
            if let Some(details) = &usage.completion_tokens_details {
                record_field_names(
                    &mut self.unknown_fields,
                    "usage.completion_tokens_details",
                    details.extra.keys(),
                );
            }
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
        self.terminal.push_answer_text(content)?;
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

        let wire_index = ToolCallAccumulator::parse_wire_index(delta.index)?;
        let content_index = if self.tools.prepare(wire_index)? {
            Some(self.allocate_content_index()?)
        } else {
            None
        };
        events.extend(self.tools.observe_delta(
            wire_index,
            content_index,
            delta,
            self.response_compat.tool_arguments,
        )?);
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
                if self.tools.is_empty() {
                    return Err(Self::protocol(
                        "finish reason tool_calls without tool call deltas",
                    ));
                }
                events.extend(self.tools.finish_all()?);
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
            || self.tools.has_open_calls()
    }

    pub(super) fn finish(&mut self) -> Result<Vec<AssistantEvent>, LlmError> {
        if !self.seen_done {
            return Err(TruncatedStreamError.into());
        }
        if !self.terminal.is_validated() {
            return Err(Self::protocol(
                "stream ended before structured output validation",
            ));
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{StreamExt as _, stream};
    use proptest::prelude::*;
    use serde_json::json;

    use crate::domain::{
        LocalRequestId, ModelRef, ProviderRequestId, ResourceLimits, StructuredSchema, TokenCount,
        ToolArguments, ToolCallId, ToolSchema, Usage, WireToolIndex,
    };
    use crate::error::StructuredOutputFailure;
    use crate::transport::{ByteStream, SseConfig};

    use super::super::stream::decode_openai_chat_stream_with_plan;
    use super::super::tool_calls::{PendingToolCall, ToolCallAccumulator};
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
        decode_openai_chat_stream_with_plan(
            body,
            context(),
            ResponseFormat::Text,
            SseConfig::default(),
            ResourceLimits::official().into(),
        )
        .collect()
        .await
    }

    async fn decode_owned(
        chunks: Vec<Result<Bytes, LlmError>>,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        let body: ByteStream = Box::pin(stream::iter(chunks));
        decode_openai_chat_stream_with_plan(
            body,
            context(),
            ResponseFormat::Text,
            SseConfig::default(),
            ResourceLimits::official().into(),
        )
        .collect()
        .await
    }

    async fn decode_with_format(
        input: &'static [u8],
        response_format: ResponseFormat,
        limits: ResponseLimits,
    ) -> Vec<Result<AssistantEvent, LlmError>> {
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(input))]));
        decode_openai_chat_stream_with_plan(
            body,
            context(),
            response_format,
            SseConfig::default(),
            limits,
        )
        .collect()
        .await
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

    fn property_config() -> ProptestConfig {
        let mut config = ProptestConfig::default();
        if std::env::var_os("PROPTEST_CASES").is_none() {
            config.cases = 96;
        }
        config
    }

    fn partition_bytes(input: &[u8], sizes: &[usize]) -> Vec<Bytes> {
        let mut chunks = Vec::new();
        let mut offset = 0;
        for size in sizes {
            if offset == input.len() {
                break;
            }
            let end = offset.saturating_add((*size).max(1)).min(input.len());
            chunks.push(Bytes::copy_from_slice(&input[offset..end]));
            offset = end;
        }
        if offset < input.len() {
            chunks.push(Bytes::copy_from_slice(&input[offset..]));
        }
        if chunks.is_empty() {
            chunks.push(Bytes::copy_from_slice(input));
        }
        chunks
    }

    #[tokio::test]
    async fn text_fixture_produces_exact_event_sequence() {
        let events = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/text.sse"
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
            include_bytes!("../../../../tests/fixtures/protocol/openai_chat/stream/usage-only.sse")
                .as_slice(),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/empty-content.sse"
            )
            .as_slice(),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/unknown-fields.sse"
            )
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
        let fixture = include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/single-call.sse"
        );
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
            "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/parallel-interleaved.sse"
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
            include_bytes!("../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/name-split.sse")
                .as_slice(),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/arguments-char-split.sse"
            )
            .as_slice(),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/id-first-chunk-only.sse"
            )
            .as_slice(),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/usage-after-tool.sse"
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
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/incomplete-arguments.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/conflicting-id.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/duplicate-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/done-before-call-end.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-finish.sse"
                ),
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
            "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/oversized-arguments.sse"
        );
        let limits = ResourceLimits::builder()
            .with_max_tool_arguments_bytes(16)
            .build()
            .unwrap();
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(fixture))]));
        let results: Vec<_> = decode_openai_chat_stream_with_plan(
            body,
            context(),
            ResponseFormat::Text,
            SseConfig::default(),
            limits.into(),
        )
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
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/single-call.sse"
            ),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/parallel-interleaved.sse"
            ),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/arguments-char-split.sse"
            ),
            include_bytes!("../../../../tests/fixtures/protocol/openai_chat/stream/text.sse"),
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/usage/usage-only-after-stop.sse"
            ),
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

    proptest! {
        #![proptest_config(property_config())]

        #[test]
        fn tool_arguments_random_split_is_stable(
            chunk_sizes in prop::collection::vec(1usize..48, 0..64),
        ) {
            let fixtures: &[&[u8]] = &[
                include_bytes!("../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/single-call.sse"),
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/parallel-interleaved.sse"
                ),
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/arguments-char-split.sse"
                ),
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/name-split.sse"
                ),
            ];

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            for fixture in fixtures {
                let baseline = runtime
                    .block_on(decode(fixture))
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let chunks = partition_bytes(fixture, &chunk_sizes)
                    .into_iter()
                    .map(Ok)
                    .collect();
                let actual = runtime
                    .block_on(decode_owned(chunks))
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                prop_assert_eq!(actual, baseline);
            }
        }
    }

    #[tokio::test]
    async fn unknown_finish_fails_closed() {
        let results = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/finish/unknown-finish-fail-closed.sse"
        ))
        .await;
        let errors: Vec<_> = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], LlmError::UnknownFinishReason(_)));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn tool_stream_preserves_parallel_calls() {
        let events = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/tool-calls/parallel-interleaved.sse"
        ))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let ends: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::ToolCallEnd { call, .. } => Some(call.name().as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ends, vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn usage_only_after_stop_merges_without_extra_text() {
        let events = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/usage/usage-only-after-stop.sse"
        ))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantEvent::TextDelta { .. }))
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantEvent::Usage(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantEvent::DetailedUsage(_)))
        );
        assert_eq!(
            events.last(),
            Some(&AssistantEvent::Done {
                finish_reason: FinishReason::Stop
            })
        );
    }

    #[tokio::test]
    async fn official_reasoning_content_is_ignored_and_usage_tokens_are_kept() {
        let events = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/thinking/official-reasoning-content-ignored.sse"
        ))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        assert!(!events.iter().any(|event| matches!(
            event,
            AssistantEvent::ThinkingStart { .. }
                | AssistantEvent::ThinkingDelta { .. }
                | AssistantEvent::ThinkingEnd { .. }
        )));
        let detailed = events
            .iter()
            .find_map(|event| match event {
                AssistantEvent::DetailedUsage(details) => Some(details),
                _ => None,
            })
            .expect("detailed usage");
        assert_eq!(detailed.reasoning_tokens(), TokenCount::Known(1));
        assert!(events.iter().any(
            |event| matches!(event, AssistantEvent::TextDelta { delta, .. } if delta == "final")
        ));
    }

    #[tokio::test]
    async fn invalid_tool_argument_json_fails_without_tool_call_end() {
        let results = decode(include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/malformed/tool-arguments-invalid-json.sse"
        ))
        .await;
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::ToolCallEnd { .. })))
        );
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
    }

    #[tokio::test]
    async fn fixture_failure_matrix_is_typed_and_terminal() {
        type ErrorCase = (&'static [u8], fn(&LlmError) -> bool);
        let cases: &[ErrorCase] = &[
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/unknown-finish-reason.sse"
                ),
                |error| matches!(error, LlmError::UnknownFinishReason(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/content-filter.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/nonzero-choice-index.sse"
                ),
                |error| matches!(error, LlmError::UnsupportedResponseSemantics(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/done-without-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/finish-without-done.sse"
                ),
                |error| matches!(error, LlmError::TruncatedStream(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/duplicate-finish.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/duplicate-done.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/data-after-done.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/json-error-object.sse"
                ),
                |error| matches!(error, LlmError::Protocol(_)),
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/malformed-json.sse"
                ),
                |error| {
                    matches!(
                        error,
                        LlmError::Protocol(inner) if inner.stage() == ErrorStage::Json
                    )
                },
            ),
            (
                include_bytes!(
                    "../../../../tests/fixtures/protocol/openai_chat/stream/truncated.sse"
                ),
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
            "../../../../tests/fixtures/protocol/openai_chat/stream/malformed-json.sse"
        ))
        .await;
        let error = results.last().unwrap().as_ref().unwrap_err();
        assert!(!error.to_string().contains("canary-private-output"));
        assert!(!format!("{error:?}").contains("canary-private-output"));
    }

    #[tokio::test]
    async fn byte_by_byte_chat_fixture_matches_single_chunk() {
        let fixture =
            include_bytes!("../../../../tests/fixtures/protocol/openai_chat/stream/text.sse");
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
    async fn structured_output_is_validated_before_done_is_emitted() {
        let invalid = include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/regression/structured-invalid-json.sse"
        );
        let results = decode_with_format(
            invalid,
            ResponseFormat::JsonObject,
            ResourceLimits::official().into(),
        )
        .await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::StructuredOutput(error)))
                if error.reason() == StructuredOutputFailure::InvalidJson
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );

        let valid = include_bytes!(
            "../../../../tests/fixtures/protocol/openai_chat/stream/regression/structured-valid.sse"
        );
        let events = decode_with_format(
            valid,
            ResponseFormat::JsonObject,
            ResourceLimits::official().into(),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
    }

    #[tokio::test]
    async fn structured_schema_violation_fails_at_done_boundary() {
        let schema = ToolSchema::new(json!({
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
            "additionalProperties": false
        }))
        .unwrap();
        let format = ResponseFormat::JsonSchema(
            StructuredSchema::new("result", None, schema, true).unwrap(),
        );
        let results = decode_with_format(
            include_bytes!(
                "../../../../tests/fixtures/protocol/openai_chat/stream/regression/structured-schema-violation.sse"
            ),
            format,
            ResourceLimits::official().into(),
        )
        .await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::StructuredOutput(error)))
                if error.reason() == StructuredOutputFailure::SchemaViolation
        ));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn structured_output_limit_fails_during_text_accumulation() {
        let mut limits: ResponseLimits = ResourceLimits::official().into();
        limits.max_structured_output_bytes = 4;
        let results = decode_with_format(
            b"data: {\"id\":\"structured\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\\"ok\\\":true}\"},\"finish_reason\":null}]}\n\n",
            ResponseFormat::JsonObject,
            limits,
        )
        .await;
        assert!(matches!(
            results.last(),
            Some(Err(LlmError::StructuredOutput(error)))
                if error.reason() == StructuredOutputFailure::TooLarge
        ));
    }

    #[tokio::test]
    async fn duplicate_tool_call_ids_fail_in_raw_state_before_any_end() {
        let results = decode_owned(vec![Ok(Bytes::from_static(
            b"data: {\"id\":\"tools\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"dup\",\"type\":\"function\",\"function\":{\"name\":\"one\",\"arguments\":\"{}\"}},{\"index\":1,\"id\":\"dup\",\"type\":\"function\",\"function\":{\"name\":\"two\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
        ))])
        .await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::ToolCallEnd { .. })))
        );
    }

    #[tokio::test]
    async fn late_duplicate_tool_call_id_fails_before_mutating_second_call() {
        let results = decode_owned(vec![
            Ok(Bytes::from_static(
                b"data: {\"id\":\"tools\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"dup\",\"type\":\"function\",\"function\":{\"name\":\"one\",\"arguments\":\"{}\"}},{\"index\":1,\"type\":\"function\",\"function\":{\"name\":\"two\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
            )),
            Ok(Bytes::from_static(
                b"data: {\"id\":\"tools\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"dup\"}]},\"finish_reason\":null}]}\n\n",
            )),
        ])
        .await;
        assert!(matches!(results.last(), Some(Err(LlmError::Protocol(_)))));
        assert!(
            !results
                .iter()
                .any(|item| matches!(item, Ok(AssistantEvent::ToolCallEnd { .. })))
        );
    }

    #[tokio::test]
    async fn empty_usage_is_ignored_without_creating_content() {
        let input = b"data: {\"id\":\"usage\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: {\"id\":\"usage\",\"model\":\"gpt-test\",\"choices\":[],\"usage\":{}}\n\ndata: [DONE]\n\n";
        let events = decode(input)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!events.iter().any(|event| matches!(
            event,
            AssistantEvent::Usage(_) | AssistantEvent::DetailedUsage(_)
        )));
        assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
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
        let limits = ResourceLimits::official().into();
        let machine_debug = format!(
            "{:?}",
            ChatStateMachine {
                context: context(),
                limits,
                started: true,
                text: ContentBlockState::NotStarted,
                refusal: ContentBlockState::NotStarted,
                tools: ToolCallAccumulator::from_pending(pending, limits, 32),
                next_content_index: 1,
                finish_reason: Some(FinishReason::ToolCalls),
                duplicate_finish_seen: false,
                seen_done: true,
                terminal: StructuredTerminal::new(
                    ResponseFormat::Text,
                    SchemaLimits {
                        max_schema_bytes: usize::MAX,
                        max_schema_depth: limits.max_schema_depth,
                        max_json_array_items: limits.max_json_array_items,
                    },
                    limits.max_structured_output_bytes,
                ),
                usage_details: None,
                generation_id: None,
                response_model: None,
                unknown_fields: BTreeSet::new(),
                response_compat: ResponseCompat::default(),
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
        let mut machine = ChatStateMachine::new_with_format(
            context(),
            ResponseFormat::Text,
            ResourceLimits::official().into(),
            ResponseCompat::default(),
        );
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
    fn unknown_field_audit_is_bounded_across_long_streams() {
        let mut machine = ChatStateMachine::new_with_format(
            context(),
            ResponseFormat::Text,
            ResourceLimits::official().into(),
            ResponseCompat::default(),
        );
        for index in 0..400 {
            let mut chunk: ChatCompletionChunkWire = serde_json::from_value(serde_json::json!({
                "id": "audit",
                "model": "gpt",
                "choices": [{"index": 0, "delta": {}, "finish_reason": null}],
            }))
            .unwrap();
            chunk
                .extra
                .insert(format!("future_{index}"), serde_json::Value::Bool(true));
            machine.record_unknown_fields(&chunk);
        }
        assert_eq!(machine.unknown_fields.len(), 256);
        assert!(machine.unknown_fields.contains("diagnostic.<truncated>"));
    }

    #[test]
    fn sse_chunk_helper_is_available_for_local_construction() {
        let encoded = sse_chunks(&[r#"{"ok":true}"#, "[DONE]"]);
        assert!(encoded.starts_with(b"data: "));
        assert!(encoded.windows(6).any(|window| window == b"[DONE]"));
    }
}
