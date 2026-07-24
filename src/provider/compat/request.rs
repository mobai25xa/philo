//! Typed request-side compatibility strategies.

use crate::domain::{
    ImageWireFormat, StreamUsagePolicy, StructuredOutputWireFormat, ThinkingWireFormat,
    ToolChoiceWireFormat,
};

/// Wire field used for the provider-neutral maximum output token intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaxOutputTokensWireFormat {
    /// Modern `OpenAI` Chat Completions `max_completion_tokens`.
    MaxCompletionTokens,
    /// Legacy compatible-provider `max_tokens`.
    MaxTokens,
}

/// Complete request encoding strategy for one resolved target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestCompat {
    /// Maximum output token field.
    pub max_output_tokens: MaxOutputTokensWireFormat,
    /// Tool-choice encoding.
    pub tool_choice: ToolChoiceWireFormat,
    /// Thinking request encoding.
    pub thinking: ThinkingWireFormat,
    /// Image request encoding.
    pub image: ImageWireFormat,
    /// Streaming usage request behavior.
    pub stream_usage: StreamUsagePolicy,
    /// Structured-output request encoding.
    pub structured_output: StructuredOutputWireFormat,
}

impl Default for RequestCompat {
    fn default() -> Self {
        Self::openai_chat_default()
    }
}

impl RequestCompat {
    /// Protocol defaults for the `OpenAI` Chat Completions driver.
    #[must_use]
    pub const fn openai_chat_default() -> Self {
        Self {
            max_output_tokens: MaxOutputTokensWireFormat::MaxCompletionTokens,
            tool_choice: ToolChoiceWireFormat::OpenAiNestedFunction,
            thinking: ThinkingWireFormat::OpenAiReasoningEffort,
            image: ImageWireFormat::OpenAiImageUrl,
            stream_usage: StreamUsagePolicy::IncludeUsage,
            structured_output: StructuredOutputWireFormat::OpenAiResponseFormat,
        }
    }
}
