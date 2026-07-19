//! Structured response formats for official Chat Completions.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use super::schema::ToolSchema;
use crate::error::{ValidationError, ValidationReason};

/// Caller-selected structured response format.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ResponseFormat {
    /// Ordinary free-form text. Official wire omits `response_format`.
    #[default]
    Text,
    /// Provider should emit a JSON object.
    JsonObject,
    /// Provider should emit JSON matching a declared schema.
    JsonSchema(StructuredSchema),
}

/// Schema-backed structured output declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuredSchema {
    name: String,
    description: Option<String>,
    schema: ToolSchema,
    strict: bool,
}

impl StructuredSchema {
    /// Creates a schema-backed structured output declaration.
    ///
    /// `name` follows the same ASCII identifier rules as tool names: `[A-Za-z0-9_-]`,
    /// `1..=64` bytes. Description, when present, must be non-empty and at most 1024
    /// UTF-8 bytes.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        schema: ToolSchema,
        strict: bool,
    ) -> Result<Self, ValidationError> {
        let name = name.into();
        validate_schema_name(&name)?;
        if let Some(description) = &description {
            if description.is_empty() {
                return Err(ValidationError::new(
                    "response_format.json_schema.description",
                    ValidationReason::Empty,
                    "structured schema description must be non-empty when provided",
                ));
            }
            if description.len() > 1024 {
                return Err(ValidationError::new(
                    "response_format.json_schema.description",
                    ValidationReason::OutOfRange,
                    "structured schema description exceeds 1024 UTF-8 bytes",
                ));
            }
        }
        if strict && !schema.is_strict_compatible() {
            return Err(ValidationError::new(
                "response_format.json_schema.strict",
                ValidationReason::OutOfRange,
                "strict structured schema must satisfy the local strict object rules",
            ));
        }
        Ok(Self {
            name,
            description,
            schema,
            strict,
        })
    }

    /// Returns the schema name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the optional schema description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the compiled schema.
    pub fn schema(&self) -> &ToolSchema {
        &self.schema
    }

    /// Returns whether the schema should be sent as strict.
    pub fn strict(&self) -> bool {
        self.strict
    }
}

fn validate_schema_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::new(
            "response_format.json_schema.name",
            ValidationReason::Empty,
            "structured schema name must not be empty",
        ));
    }
    if name.len() > 64 {
        return Err(ValidationError::new(
            "response_format.json_schema.name",
            ValidationReason::OutOfRange,
            "structured schema name exceeds 64 bytes",
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ValidationError::new(
            "response_format.json_schema.name",
            ValidationReason::InvalidIdentifier,
            "structured schema name contains an unsupported character",
        ));
    }
    Ok(())
}
