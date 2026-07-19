//! Streaming assistant events and the single completion collector.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::struct_excessive_bools,
    clippy::struct_field_names
)]

use futures_util::{Stream, StreamExt};

use super::ModelRef;
pub use super::ids::{GenerationId, LocalRequestId, ProviderRequestId};
use crate::error::{LlmError, ProtocolError, TruncatedStreamError};

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
    /// Tool-call completion (reserved for later protocol support).
    ToolCalls,
    /// Provider-specific value retained without claiming a known success reason.
    Unknown(String),
}

/// A public event emitted by the streaming state machine.
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
    /// Starts the phase-one text content.
    TextStart {
        /// Content index; phase one accepts only index zero.
        index: usize,
    },
    /// Appends a Unicode text fragment.
    TextDelta {
        /// Content index; phase one accepts only index zero.
        index: usize,
        /// Unmodified text fragment.
        delta: String,
    },
    /// Ends the phase-one text content.
    TextEnd {
        /// Content index; phase one accepts only index zero.
        index: usize,
    },
    /// Reports token usage; absence means unknown.
    Usage(Usage),
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
    text: String,
    usage: Option<Usage>,
    finish_reason: FinishReason,
    local_request_id: Option<LocalRequestId>,
    provider_request_id: Option<ProviderRequestId>,
    generation_id: Option<GenerationId>,
    model: Option<ModelRef>,
}
impl AssistantMessage {
    /// Returns collected text.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns usage, or `None` when the provider omitted it.
    pub fn usage(&self) -> Option<Usage> {
        self.usage
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
pub async fn collect_assistant_message<S>(stream: S) -> Result<AssistantMessage, LlmError>
where
    S: Stream<Item = Result<AssistantEvent, LlmError>>,
{
    let mut stream = Box::pin(stream);
    let mut state = Collector::default();
    while let Some(item) = stream.next().await {
        state.accept(item?)?;
    }
    state.finish().map_err(Into::into)
}

#[derive(Default)]
struct Collector {
    started: bool,
    text_started: bool,
    text_ended: bool,
    done: bool,
    text: String,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
    local_request_id: Option<LocalRequestId>,
    provider_request_id: Option<ProviderRequestId>,
    generation_id: Option<GenerationId>,
}
impl Collector {
    fn protocol(message: impl Into<String>) -> LlmError {
        ProtocolError::new(message).into()
    }
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
                if self.started {
                    return Err(Self::protocol("duplicate Start"));
                }
                self.started = true;
                self.local_request_id = Some(local_request_id);
                self.provider_request_id = provider_request_id;
                self.generation_id = generation_id;
            }
            AssistantEvent::TextStart { index } => {
                if index != 0 || self.text_started || self.text_ended {
                    return Err(Self::protocol("invalid TextStart sequence"));
                }
                self.text_started = true;
            }
            AssistantEvent::TextDelta { index, delta } => {
                if index != 0 || !self.text_started || self.text_ended {
                    return Err(Self::protocol("invalid TextDelta sequence"));
                }
                self.text.push_str(&delta);
            }
            AssistantEvent::TextEnd { index } => {
                if index != 0 || !self.text_started || self.text_ended {
                    return Err(Self::protocol("invalid TextEnd sequence"));
                }
                self.text_ended = true;
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
            AssistantEvent::Done { finish_reason } => {
                if !self.text_started || !self.text_ended {
                    return Err(Self::protocol(
                        "Done requires TextStart followed by TextEnd",
                    ));
                }
                self.done = true;
                self.finish_reason = Some(finish_reason);
            }
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
        Ok(AssistantMessage {
            text: self.text,
            usage: self.usage,
            finish_reason,
            local_request_id: self.local_request_id,
            provider_request_id: self.provider_request_id,
            generation_id: self.generation_id,
            model: None,
        })
    }
}
