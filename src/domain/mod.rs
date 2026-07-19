//! Provider-independent domain types.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod event;
pub mod ids;
pub mod request;

pub use event::{AssistantEvent, AssistantMessage, FinishReason, Usage, collect_assistant_message};
pub use ids::{GenerationId, LocalRequestId, ProviderRequestId, TraceId};
pub use request::{
    CapabilitySet, CapabilityStatus, GenerateRequest, GenerationOptions, LlmRequest,
    RequestMetadata, RequestTimeout,
};

use std::fmt;

/// A validated provider identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(String);

/// A validated protocol identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProtocolId(String);

/// A validated model identifier. Internal whitespace is preserved.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelId(String);

macro_rules! id_impl {
    ($name:ident, $label:literal) => {
        impl $name {
            /// Creates an identifier without trimming or normalizing it.
            pub fn new(value: impl Into<String>) -> Result<Self, crate::error::ValidationError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(crate::error::ValidationError::new(
                        stringify!($name),
                        crate::error::ValidationReason::Empty,
                        concat!($label, " must not be empty"),
                    ));
                }
                if value.trim() != value {
                    return Err(crate::error::ValidationError::new(
                        stringify!($name),
                        crate::error::ValidationReason::BoundaryWhitespace,
                        concat!($label, " must not have leading or trailing whitespace"),
                    ));
                }
                Ok(Self(value))
            }
            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// Consumes the identifier and returns its string value.
            pub fn into_inner(self) -> String {
                self.0
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
id_impl!(ProviderId, "provider id");
id_impl!(ProtocolId, "protocol id");
id_impl!(ModelId, "model id");

/// Provider and model selected for a generation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelRef {
    provider: ProviderId,
    model: ModelId,
}

impl ModelRef {
    /// Creates a model reference from validated identifiers.
    pub fn from_ids(provider: ProviderId, model: ModelId) -> Self {
        Self { provider, model }
    }
    /// Creates a model reference from string identifiers.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, crate::error::ValidationError> {
        Ok(Self::from_ids(
            ProviderId::new(provider)?,
            ModelId::new(model)?,
        ))
    }
    /// Returns the provider identifier.
    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }
    /// Returns the model identifier.
    pub fn model(&self) -> &ModelId {
        &self.model
    }
}

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

/// Provider-independent content part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentPart {
    /// Text content preserved exactly as supplied.
    Text {
        /// Unmodified UTF-8 text.
        text: String,
    },
}

impl ContentPart {
    /// Creates a text part while preserving the text exactly.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
    /// Returns text for this part.
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text { text } => text,
        }
    }
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
