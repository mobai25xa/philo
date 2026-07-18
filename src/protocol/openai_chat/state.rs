use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;

use super::wire::{ChatCompletionChunkWire, ChoiceWire, UsageWire};
use crate::domain::{
    AssistantEvent, FinishReason, GenerationId, LocalRequestId, ModelRef, ProviderRequestId, Usage,
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

/// Converts an SDK byte stream into phase-one assistant events.
pub(crate) fn decode_openai_chat_stream(
    body: ByteStream,
    context: OpenAiChatStreamContext,
) -> OpenAiChatEventStream {
    OpenAiChatEventStream {
        source: SseDecoder::new(body),
        machine: ChatStateMachine::new(context),
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
    started: bool,
    text: TextState,
    finish_reason: Option<FinishReason>,
    seen_done: bool,
    usage: Option<Usage>,
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
            .field("finish_seen", &self.finish_reason.is_some())
            .field("done_seen", &self.seen_done)
            .field("usage_seen", &self.usage.is_some())
            .field("generation_id_seen", &self.generation_id.is_some())
            .field("response_model_seen", &self.response_model.is_some())
            .field("unknown_field_count", &self.unknown_fields.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextState {
    NotStarted,
    Open,
    Closed,
}

impl ChatStateMachine {
    fn new(context: OpenAiChatStreamContext) -> Self {
        Self {
            context,
            started: false,
            text: TextState::NotStarted,
            finish_reason: None,
            seen_done: false,
            usage: None,
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
        self.record_unknown_fields(&chunk);

        let prepared = PreparedChunk::validate(&chunk, self.finish_reason.is_some())?;
        self.observe_identity(chunk.id.as_deref(), chunk.model.as_deref())?;

        let mut events = Vec::new();
        self.emit_start(&mut events);

        if let Some(choice) = chunk.choices.first() {
            self.apply_choice(choice, prepared.finish_reason, &mut events)?;
        }
        if let Some(usage) = prepared.usage {
            if let Some(previous) = self.usage {
                if previous != usage {
                    return Err(Self::protocol("conflicting duplicate usage"));
                }
            } else {
                self.usage = Some(usage);
                events.push(AssistantEvent::Usage(usage));
            }
        }
        Ok(events)
    }

    fn record_unknown_fields(&mut self, chunk: &ChatCompletionChunkWire) {
        record_field_names(&mut self.unknown_fields, "chunk", chunk.extra.keys());
        for choice in &chunk.choices {
            record_field_names(&mut self.unknown_fields, "choice", choice.extra.keys());
            if let Some(delta) = &choice.delta {
                record_field_names(&mut self.unknown_fields, "delta", delta.extra.keys());
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
        if let Some(content) = choice
            .delta
            .as_ref()
            .and_then(|delta| delta.content.as_deref())
            && !content.is_empty()
        {
            if self.text == TextState::Closed {
                return Err(Self::protocol("text delta received after finish reason"));
            }
            if self.text == TextState::NotStarted {
                self.text = TextState::Open;
                events.push(AssistantEvent::TextStart { index: 0 });
            }
            events.push(AssistantEvent::TextDelta {
                index: 0,
                delta: content.to_owned(),
            });
        }

        if let Some(reason) = finish_reason {
            if self.finish_reason.is_some() {
                return Err(Self::protocol("duplicate finish reason"));
            }
            if self.text == TextState::NotStarted {
                self.text = TextState::Open;
                events.push(AssistantEvent::TextStart { index: 0 });
            }
            if self.text == TextState::Open {
                self.text = TextState::Closed;
                events.push(AssistantEvent::TextEnd { index: 0 });
            }
            self.finish_reason = Some(reason);
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<AssistantEvent>, LlmError> {
        if !self.seen_done {
            return Err(TruncatedStreamError.into());
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
    usage: Option<Usage>,
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
        let usage = chunk.usage.as_ref().map(validate_usage).transpose()?;
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
            if delta.tool_calls.is_some() {
                return Err(UnsupportedResponseSemantics::new("tool_calls").into());
            }
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

fn validate_usage(wire: &UsageWire) -> Result<Usage, LlmError> {
    let input = u64::try_from(wire.prompt_tokens)
        .map_err(|_| ChatStateMachine::protocol("usage token count must be non-negative"))?;
    let output = u64::try_from(wire.completion_tokens)
        .map_err(|_| ChatStateMachine::protocol("usage token count must be non-negative"))?;
    let total = u64::try_from(wire.total_tokens)
        .map_err(|_| ChatStateMachine::protocol("usage token count must be non-negative"))?;
    Usage::new(input, output, total).map_err(Into::into)
}

fn parse_finish_reason(raw: &str) -> Result<FinishReason, LlmError> {
    match raw {
        "stop" => Ok(FinishReason::Stop),
        "length" => Ok(FinishReason::Length),
        "content_filter" => Ok(FinishReason::ContentFilter),
        "tool_calls" | "function_call" => Err(UnsupportedResponseSemantics::new(raw).into()),
        _ => Err(UnknownFinishReason::new(bounded_label(raw, 64)).into()),
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

    #[tokio::test]
    async fn text_fixture_produces_exact_event_sequence() {
        let events = decode(include_bytes!(
            "../../../tests/fixtures/responses/openai_chat/text.sse"
        ))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        assert_eq!(events.len(), 7);
        assert!(matches!(
            &events[0],
            AssistantEvent::Start {
                provider_request_id: Some(id),
                generation_id: Some(generation),
                ..
            } if id.as_str() == "header-request-id" && generation.as_str() == "chatcmpl-text"
        ));
        assert_eq!(events[1], AssistantEvent::TextStart { index: 0 });
        assert_eq!(
            events[2],
            AssistantEvent::TextDelta {
                index: 0,
                delta: "Hel".to_owned()
            }
        );
        assert_eq!(
            events[3],
            AssistantEvent::TextDelta {
                index: 0,
                delta: "lo".to_owned()
            }
        );
        assert_eq!(events[4], AssistantEvent::TextEnd { index: 0 });
        assert_eq!(
            events[5],
            AssistantEvent::Usage(Usage::new(2, 1, 3).unwrap())
        );
        assert_eq!(
            events[6],
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
                include_bytes!("../../../tests/fixtures/responses/openai_chat/tool-finish.sse"),
                |error| matches!(error, LlmError::UnsupportedResponseSemantics(_)),
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
                |error| matches!(error, LlmError::Protocol(inner) if inner.stage() == ErrorStage::Json),
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

    #[test]
    fn unknown_field_audit_records_names_without_values() {
        let chunk: ChatCompletionChunkWire = serde_json::from_str(
            r#"{"id":"audit","model":"gpt","top_future":"canary-value","choices":[{"index":0,"choice_future":1,"delta":{"delta_future":true},"finish_reason":null}],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0,"usage_future":"private"}}"#,
        )
        .unwrap();
        let mut machine = ChatStateMachine::new(context());
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
}
