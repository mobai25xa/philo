//! Provider-independent conversation messages.
#![allow(clippy::must_use_candidate)]

use super::ContentPart;

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
}

/// A provider-independent message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    role: MessageRole,
    content: Vec<ContentPart>,
}

impl Message {
    /// Creates a message with any number of content parts.
    pub fn new(role: MessageRole, content: Vec<ContentPart>) -> Self {
        Self { role, content }
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

    fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentPart::text(text)])
    }

    /// Returns the message role.
    pub fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns all content parts.
    pub fn content(&self) -> &[ContentPart] {
        &self.content
    }

    /// Returns mutable content storage for local assembly.
    pub fn content_mut(&mut self) -> &mut Vec<ContentPart> {
        &mut self.content
    }
}
