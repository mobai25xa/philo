//! Strongly typed identifiers shared across domain and request lifecycle boundaries.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use crate::error::{ValidationError, ValidationReason};

macro_rules! validated_id {
    ($name:ident, $description:literal, $label:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Creates an identifier without trimming or normalizing it.
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                let reason = if value.is_empty() {
                    Some(ValidationReason::Empty)
                } else if value.trim() != value {
                    Some(ValidationReason::BoundaryWhitespace)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    return Err(ValidationError::new(
                        stringify!($name),
                        reason,
                        concat!($label, " must be non-empty and have no boundary whitespace"),
                    ));
                }
                Ok(Self(value))
            }

            /// Returns the identifier.
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

validated_id!(
    ProviderId,
    "A validated provider identifier.",
    "provider id"
);
validated_id!(
    ProtocolId,
    "A validated protocol identifier.",
    "protocol id"
);
validated_id!(
    ModelId,
    "A validated model identifier. Internal whitespace is preserved.",
    "model id"
);
validated_id!(
    LocalRequestId,
    "Identifier allocated by philo for one local request attempt.",
    "lifecycle id"
);
validated_id!(
    ProviderRequestId,
    "Identifier returned in provider response headers.",
    "lifecycle id"
);
validated_id!(
    GenerationId,
    "Identifier returned by the generation protocol body.",
    "lifecycle id"
);
validated_id!(
    TraceId,
    "Application telemetry identifier that may correlate multiple SDK requests.",
    "lifecycle id"
);

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
    ) -> Result<Self, ValidationError> {
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

/// Position of a provider-independent content block within one assistant generation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentIndex(u32);

impl ContentIndex {
    /// Creates a content index.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the numeric index.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for ContentIndex {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

/// Protocol-local tool-call index within one streamed response.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WireToolIndex(u32);

impl WireToolIndex {
    /// Creates a wire tool index.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the numeric index.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for WireToolIndex {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

/// Stable identifier pairing a tool call with its result.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ToolCallId(String);

impl ToolCallId {
    /// Maximum UTF-8 byte length accepted by the domain contract.
    pub const MAX_BYTES: usize = 256;

    /// Creates a non-empty tool call identifier while preserving provider text exactly.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::new(
                "tool_call_id",
                ValidationReason::Empty,
                "tool call id must not be empty",
            ));
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ValidationError::new(
                "tool_call_id",
                ValidationReason::OutOfRange,
                "tool call id exceeds the domain byte limit",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the provider-preserved identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the identifier.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ToolCallId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Validated tool name shared by declarations, calls, and results.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ToolName(String);

impl ToolName {
    /// Maximum ASCII byte length accepted by the protocol contract.
    pub const MAX_BYTES: usize = 64;

    /// Creates a tool name matching `[A-Za-z0-9_-]` and `1..=64` bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::new(
                "tool_name",
                ValidationReason::Empty,
                "tool name must not be empty",
            ));
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ValidationError::new(
                "tool_name",
                ValidationReason::OutOfRange,
                "tool name exceeds 64 bytes",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ValidationError::new(
                "tool_name",
                ValidationReason::InvalidIdentifier,
                "tool name contains an unsupported character",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
