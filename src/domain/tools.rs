//! Provider-independent tool declarations and completed tool calls.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

use super::request::{CapabilitySet, CapabilityStatus};
use super::schema::ToolSchema;
use super::{ToolCallId, ToolName};
use crate::error::{
    CapabilityError, LlmError, SchemaError, SchemaFailure, ValidationError, ValidationReason,
};

/// Official phase-two tool list limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolLimits {
    /// Maximum number of tools declared on one request.
    pub max_tools: usize,
    /// Maximum UTF-8 byte length of one tool description.
    pub max_tool_description_bytes: usize,
}

impl ToolLimits {
    /// Official `OpenAI` profile defaults frozen for phase two.
    pub const fn official() -> Self {
        Self {
            max_tools: 128,
            max_tool_description_bytes: 1024,
        }
    }
}

/// Complete JSON arguments for a tool call.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolArguments {
    raw_json: String,
    value: Value,
}

impl ToolArguments {
    /// Parses complete JSON and preserves the original representation.
    pub fn from_raw_json(raw_json: impl Into<String>) -> Result<Self, serde_json::Error> {
        let raw_json = raw_json.into();
        let value = serde_json::from_str(&raw_json)?;
        Ok(Self { raw_json, value })
    }

    /// Creates arguments from a JSON value using its canonical serde representation.
    pub fn from_value(value: Value) -> Self {
        let raw_json = value.to_string();
        Self { raw_json, value }
    }

    /// Returns the exact complete JSON received from the protocol or caller.
    pub fn raw_json(&self) -> &str {
        &self.raw_json
    }

    /// Returns the parsed JSON value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Consumes the arguments into their preserved and parsed forms.
    pub fn into_parts(self) -> (String, Value) {
        (self.raw_json, self.value)
    }
}

impl fmt::Debug for ToolArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolArguments")
            .field("raw_json_bytes", &self.raw_json.len())
            .field("value_kind", &json_kind(&self.value))
            .finish_non_exhaustive()
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A complete tool call. Construction requires an ID, a validated name, and complete JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    id: ToolCallId,
    name: ToolName,
    arguments: ToolArguments,
}

impl ToolCall {
    /// Creates a completed tool call.
    pub fn new(id: ToolCallId, name: ToolName, arguments: ToolArguments) -> Self {
        Self {
            id,
            name,
            arguments,
        }
    }

    /// Returns the stable call identifier.
    pub fn id(&self) -> &ToolCallId {
        &self.id
    }

    /// Returns the validated tool name.
    pub fn name(&self) -> &ToolName {
        &self.name
    }

    /// Returns complete arguments.
    pub fn arguments(&self) -> &ToolArguments {
        &self.arguments
    }

    /// Consumes the call into its components.
    pub fn into_parts(self) -> (ToolCallId, ToolName, ToolArguments) {
        (self.id, self.name, self.arguments)
    }
}

/// A provider-independent tool declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDefinition {
    name: ToolName,
    description: Option<String>,
    parameters: ToolSchema,
    strict: bool,
}

impl ToolDefinition {
    /// Creates a tool declaration with a validated schema.
    pub fn new(name: ToolName, parameters: ToolSchema) -> Self {
        Self {
            name,
            description: None,
            parameters,
            strict: false,
        }
    }

    /// Sets an optional description.
    pub fn with_description(
        mut self,
        description: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let description = description.into();
        if description.len() > ToolLimits::official().max_tool_description_bytes {
            return Err(ValidationError::new(
                "tools.description",
                ValidationReason::OutOfRange,
                "tool description exceeds the allowed byte limit",
            ));
        }
        self.description = Some(description);
        Ok(self)
    }

    /// Requests strict schema enforcement when the selected model supports it.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Returns the tool name.
    pub fn name(&self) -> &ToolName {
        &self.name
    }

    /// Returns the optional description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the validated parameters schema.
    pub fn parameters(&self) -> &ToolSchema {
        &self.parameters
    }

    /// Returns whether strict mode was requested by the caller.
    pub fn strict(&self) -> bool {
        self.strict
    }
}

/// Domain tool selection strategy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolChoice {
    /// Let the model decide whether to call tools.
    Auto,
    /// Force the model not to call tools.
    None,
    /// Require the model to call at least one tool.
    Required,
    /// Force the model to call one named tool.
    Specific {
        /// Existing tool name.
        name: ToolName,
    },
}

/// Whether the model may emit parallel tool calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelToolCalls {
    /// Allow parallel tool calls.
    Enabled,
    /// Disallow parallel tool calls.
    Disabled,
}

/// Validates tool declarations and selection before they enter request encoding.
pub fn validate_tool_options(
    tools: &[ToolDefinition],
    tool_choice: Option<&ToolChoice>,
    parallel_tool_calls: Option<ParallelToolCalls>,
    capabilities: &CapabilitySet,
) -> Result<(), LlmError> {
    validate_tool_options_with_limits(
        tools,
        tool_choice,
        parallel_tool_calls,
        capabilities,
        ToolLimits::official(),
    )
}

/// Validates tool declarations under explicit limits.
pub fn validate_tool_options_with_limits(
    tools: &[ToolDefinition],
    tool_choice: Option<&ToolChoice>,
    parallel_tool_calls: Option<ParallelToolCalls>,
    capabilities: &CapabilitySet,
    limits: ToolLimits,
) -> Result<(), LlmError> {
    if tools.len() > limits.max_tools {
        return Err(ValidationError::new(
            "tools",
            ValidationReason::OutOfRange,
            "tool count exceeds the allowed limit",
        )
        .into());
    }

    if !tools.is_empty() {
        check_capability("tools", "function_tools", capabilities.function_tools)?;
    }

    let mut names = BTreeSet::new();
    for (index, tool) in tools.iter().enumerate() {
        if !names.insert(tool.name().as_str()) {
            return Err(ValidationError::new(
                format!("tools[{index}].name"),
                ValidationReason::DuplicateToolName,
                "tool names must be unique within one request",
            )
            .into());
        }
        if let Some(description) = tool.description()
            && description.len() > limits.max_tool_description_bytes
        {
            return Err(ValidationError::new(
                format!("tools[{index}].description"),
                ValidationReason::OutOfRange,
                "tool description exceeds the allowed byte limit",
            )
            .into());
        }
        if tool.strict() {
            check_capability(
                &format!("tools[{index}].strict"),
                "strict_tools",
                capabilities.strict_tools,
            )?;
            if !tool.parameters().is_strict_compatible() {
                let reason = if tool.parameters().value().get("additionalProperties")
                    == Some(&serde_json::Value::Bool(false))
                {
                    SchemaFailure::StrictPropertyNotRequired
                } else {
                    SchemaFailure::StrictObjectMissingAdditionalPropertiesFalse
                };
                return Err(SchemaError::new(
                    format!("tools[{index}].parameters"),
                    reason,
                    Some("#".to_owned()),
                    "strict tools require a strict-compatible object schema",
                )
                .into());
            }
        }
    }

    if let Some(choice) = tool_choice {
        match choice {
            ToolChoice::Auto => {
                if tools.is_empty() {
                    return Err(ValidationError::new(
                        "tool_choice",
                        ValidationReason::EmptyToolList,
                        "auto tool choice requires at least one tool",
                    )
                    .into());
                }
            }
            ToolChoice::None => {}
            ToolChoice::Required => {
                if tools.is_empty() {
                    return Err(ValidationError::new(
                        "tool_choice",
                        ValidationReason::EmptyToolList,
                        "required tool choice requires at least one tool",
                    )
                    .into());
                }
                check_capability(
                    "tool_choice",
                    "tool_choice_required",
                    capabilities.tool_choice_required,
                )?;
            }
            ToolChoice::Specific { name } => {
                if tools.is_empty() {
                    return Err(ValidationError::new(
                        "tool_choice",
                        ValidationReason::EmptyToolList,
                        "specific tool choice requires at least one tool",
                    )
                    .into());
                }
                check_capability(
                    "tool_choice",
                    "tool_choice_specific",
                    capabilities.tool_choice_specific,
                )?;
                if !tools.iter().any(|tool| tool.name() == name) {
                    return Err(ValidationError::new(
                        "tool_choice.name",
                        ValidationReason::UnknownTool,
                        "specific tool choice must reference a declared tool",
                    )
                    .into());
                }
            }
        }
    }

    if let Some(parallel) = parallel_tool_calls {
        let _ = parallel;
        check_capability(
            "parallel_tool_calls",
            "parallel_tool_calls",
            capabilities.parallel_tool_calls,
        )?;
        if tools.is_empty() {
            return Err(ValidationError::new(
                "parallel_tool_calls",
                ValidationReason::EmptyToolList,
                "parallel_tool_calls requires at least one tool",
            )
            .into());
        }
    }

    Ok(())
}

fn check_capability(
    field: &str,
    capability: &str,
    status: CapabilityStatus,
) -> Result<(), LlmError> {
    match status {
        CapabilityStatus::Supported => Ok(()),
        CapabilityStatus::Unsupported => {
            Err(CapabilityError::new(field, capability, "Unsupported").into())
        }
        CapabilityStatus::Unknown => Err(CapabilityError::new(field, capability, "Unknown").into()),
    }
}
