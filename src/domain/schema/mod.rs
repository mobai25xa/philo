//! Controlled JSON Schema compilation for tools and structured output.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

mod budget;
mod compile;
mod reference;
mod validate;

use serde_json::Value;

use crate::error::{SchemaError, ToolValidationError};
pub use budget::SchemaLimits;

/// A provider-independent tool or response schema with controlled validation metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolSchema {
    value: Value,
    strict_compatible: bool,
}

impl ToolSchema {
    /// Compiles and validates a local schema subset. No network access is performed.
    pub fn new(value: Value) -> Result<Self, SchemaError> {
        Self::with_limits(value, SchemaLimits::official())
    }

    /// Compiles a schema under the provided limits.
    pub fn with_limits(value: Value, limits: SchemaLimits) -> Result<Self, SchemaError> {
        let metadata = compile::compile_schema(&value, limits)?;
        Ok(Self {
            value,
            strict_compatible: metadata.strict_compatible,
        })
    }

    /// Returns the preserved schema value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Returns whether the schema satisfies the local strict-mode object rules.
    pub fn is_strict_compatible(&self) -> bool {
        self.strict_compatible
    }

    /// Validates a complete JSON instance against this schema.
    ///
    /// Failures never include argument values or secret material. Only field
    /// paths and stable reason codes are retained.
    pub fn validate_instance(
        &self,
        instance: &Value,
        limits: SchemaLimits,
    ) -> Result<(), ToolValidationError> {
        validate::validate_instance(&self.value, instance, limits)
    }
}

impl std::fmt::Debug for ToolSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSchema")
            .field("value_kind", &compile::value_kind(&self.value))
            .field("strict_compatible", &self.strict_compatible)
            .finish_non_exhaustive()
    }
}
