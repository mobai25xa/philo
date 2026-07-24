//! Typed response-side compatibility strategies.

/// Streamed finish-reason handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinishReasonCompat {
    /// Fail closed on values outside the `OpenAI` Chat vocabulary.
    StrictOpenAi,
    /// Accept one payload-free repeat of the already observed finish reason.
    ///
    /// A different reason, another repeat, or any late delta remains a
    /// protocol error. This is intended for reviewed gateway terminal chunks
    /// that repeat the normalized finish reason while attaching final usage.
    AllowOneIdenticalDuplicate,
}

/// Accepted streamed tool-argument representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolArgumentsCompat {
    /// Require JSON arguments to arrive as a string accumulator.
    JsonString,
    /// Accept a string or object and normalize privately in the adapter.
    StringOrObject,
}

/// Usage object interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageCompat {
    /// `OpenAI` prompt/completion/total token semantics.
    OpenAi,
    /// Preserve `OpenAI` core counters but discard an invalid reasoning subset.
    ///
    /// This applies only when a gateway reports `reasoning_tokens` greater
    /// than `completion_tokens`. Negative values and inconsistent core totals
    /// remain protocol errors.
    OpenAiDropInconsistentReasoning,
}

/// In-stream provider error handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineErrorCompat {
    /// Treat an error object as a terminal protocol failure.
    Reject,
}

/// Complete response decoding strategy for one resolved target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseCompat {
    /// Finish-reason handling.
    pub finish_reason: FinishReasonCompat,
    /// Tool argument handling.
    pub tool_arguments: ToolArgumentsCompat,
    /// Usage handling.
    pub usage: UsageCompat,
    /// Inline error handling.
    pub inline_error: InlineErrorCompat,
}

impl Default for ResponseCompat {
    fn default() -> Self {
        Self::openai_chat_default()
    }
}

impl ResponseCompat {
    /// Protocol defaults for the `OpenAI` Chat Completions state machine.
    #[must_use]
    pub const fn openai_chat_default() -> Self {
        Self {
            finish_reason: FinishReasonCompat::StrictOpenAi,
            tool_arguments: ToolArgumentsCompat::JsonString,
            usage: UsageCompat::OpenAi,
            inline_error: InlineErrorCompat::Reject,
        }
    }
}
