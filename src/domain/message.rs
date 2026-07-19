//! Provider-independent conversation messages.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use super::{ContentPart, GenerationId, ToolCallId, ToolName};
use crate::error::{HistoryError, HistoryFailure};

/// Role of a message in a conversation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessageRole {
    /// Instruction supplied by an application developer.
    Developer,
    /// System-level instruction.
    System,
    /// End-user input.
    User,
    /// Prior assistant output.
    Assistant,
    /// Tool-result payload paired with a prior tool call.
    Tool,
}

/// Application-authored result for one completed tool call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResultMessage {
    tool_call_id: ToolCallId,
    tool_name: ToolName,
    content: Vec<ContentPart>,
    is_error: bool,
    source_generation_id: Option<GenerationId>,
}

impl ToolResultMessage {
    /// Creates a successful tool result with one non-empty text part.
    pub fn text(
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        text: impl Into<String>,
    ) -> Result<Self, HistoryError> {
        Self::new(
            tool_call_id,
            tool_name,
            vec![ContentPart::text(text)],
            false,
            None,
        )
    }

    /// Creates an error tool result with a safe non-empty text summary.
    pub fn error_text(
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        text: impl Into<String>,
    ) -> Result<Self, HistoryError> {
        Self::new(
            tool_call_id,
            tool_name,
            vec![ContentPart::text(text)],
            true,
            None,
        )
    }

    /// Creates a tool result and enforces Official P2 content constraints.
    pub fn new(
        tool_call_id: ToolCallId,
        tool_name: ToolName,
        content: Vec<ContentPart>,
        is_error: bool,
        source_generation_id: Option<GenerationId>,
    ) -> Result<Self, HistoryError> {
        validate_official_tool_result_content(&content)?;
        Ok(Self {
            tool_call_id,
            tool_name,
            content,
            is_error,
            source_generation_id,
        })
    }

    /// Returns the paired tool call identifier.
    pub fn tool_call_id(&self) -> &ToolCallId {
        &self.tool_call_id
    }

    /// Returns the tool name for diagnostics. Pairing uses the call id.
    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    /// Returns the tool result content.
    pub fn content(&self) -> &[ContentPart] {
        &self.content
    }

    /// Returns whether the application marked this result as an error.
    pub fn is_error(&self) -> bool {
        self.is_error
    }

    /// Returns the optional source generation identifier.
    pub fn source_generation_id(&self) -> Option<&GenerationId> {
        self.source_generation_id.as_ref()
    }
}

fn validate_official_tool_result_content(content: &[ContentPart]) -> Result<(), HistoryError> {
    if content.len() != 1 {
        return Err(HistoryError::new(
            "tool_result.content",
            HistoryFailure::UnsupportedContent,
            None,
            "official tool results require exactly one content part",
        ));
    }
    match &content[0] {
        ContentPart::Text { text } if !text.is_empty() => Ok(()),
        ContentPart::Text { .. } => Err(HistoryError::new(
            "tool_result.content",
            HistoryFailure::UnsupportedContent,
            Some("content[0]".to_owned()),
            "official tool results require non-empty text",
        )),
        ContentPart::Image(_)
        | ContentPart::Thinking(_)
        | ContentPart::Refusal(_)
        | ContentPart::ToolCall(_) => Err(HistoryError::new(
            "tool_result.content",
            HistoryFailure::UnsupportedContent,
            Some("content[0]".to_owned()),
            "official tool results only accept a single text part",
        )),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MessagePayload {
    Content(Vec<ContentPart>),
    ToolResult(ToolResultMessage),
}

/// A provider-independent message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    role: MessageRole,
    payload: MessagePayload,
}

impl Message {
    /// Creates a message with any number of content parts.
    pub fn new(role: MessageRole, content: Vec<ContentPart>) -> Self {
        debug_assert!(
            role != MessageRole::Tool,
            "use Message::from_tool_result for tool role messages"
        );
        Self {
            role,
            payload: MessagePayload::Content(content),
        }
    }

    /// Creates a developer message.
    pub fn developer(text: impl Into<String>) -> Self {
        Self::text(MessageRole::Developer, text)
    }

    /// Creates a system message.
    pub fn system(text: impl Into<String>) -> Self {
        Self::text(MessageRole::System, text)
    }

    /// Creates a user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self::text(MessageRole::User, text)
    }

    /// Creates an assistant message.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(MessageRole::Assistant, text)
    }

    /// Creates the only supported Tool-role message from a tool result.
    pub fn from_tool_result(result: ToolResultMessage) -> Self {
        Self {
            role: MessageRole::Tool,
            payload: MessagePayload::ToolResult(result),
        }
    }

    fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentPart::text(text)])
    }

    /// Returns the message role.
    pub fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns content parts for non-tool messages.
    ///
    /// Tool-result messages return an empty slice; use [`Self::tool_result`].
    pub fn content(&self) -> &[ContentPart] {
        match &self.payload {
            MessagePayload::Content(content) => content,
            MessagePayload::ToolResult(_) => &[],
        }
    }

    /// Returns a tool-result payload when this message has the Tool role.
    pub fn tool_result(&self) -> Option<&ToolResultMessage> {
        match &self.payload {
            MessagePayload::ToolResult(result) => Some(result),
            MessagePayload::Content(_) => None,
        }
    }
}
