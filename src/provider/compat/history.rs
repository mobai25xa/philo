//! Typed history compatibility strategies.

use crate::domain::{
    MissingToolResultPolicy, ThinkingReplayPolicy, ToolCallIdPolicy, ToolResultNamePolicy,
    UnsupportedContentPolicy,
};

/// Complete history normalization strategy for one resolved target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryCompat {
    /// Missing tool-result behavior.
    pub missing_tool_result: MissingToolResultPolicy,
    /// Unsupported content behavior.
    pub unsupported_content: UnsupportedContentPolicy,
    /// Thinking replay behavior.
    pub thinking_replay: ThinkingReplayPolicy,
    /// Tool-result name encoding.
    pub tool_result_name: ToolResultNamePolicy,
    /// Tool-call identifier normalization.
    pub tool_call_id: ToolCallIdPolicy,
}

impl HistoryCompat {
    /// Protocol defaults for `OpenAI` Chat history.
    #[must_use]
    pub const fn openai_chat_default() -> Self {
        Self {
            missing_tool_result: MissingToolResultPolicy::Reject,
            unsupported_content: UnsupportedContentPolicy::Reject,
            thinking_replay: ThinkingReplayPolicy::DropAll,
            tool_result_name: ToolResultNamePolicy::Omit,
            tool_call_id: ToolCallIdPolicy::OpenAi,
        }
    }
}
