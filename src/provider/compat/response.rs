//! Typed response-side compatibility strategies.

/// Accepted finish-reason vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinishReasonCompat {
    /// Fail closed on values outside the `OpenAI` Chat vocabulary.
    StrictOpenAi,
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
