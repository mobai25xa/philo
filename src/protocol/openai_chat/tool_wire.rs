//! Private `OpenAI` function-tool request wire types.

use serde::Serialize;
use serde_json::Value;

use crate::domain::{
    CapabilitySet, CapabilityStatus, ParallelToolCalls, ToolChoice, ToolDefinition,
};
use crate::error::{CapabilityError, LlmError, ProtocolError};

#[derive(Clone, Copy, Serialize)]
enum FunctionKindWire {
    #[serde(rename = "function")]
    Function,
}

#[derive(Serialize)]
pub(super) struct ToolWire<'a> {
    #[serde(rename = "type")]
    kind: FunctionKindWire,
    function: FunctionDefinitionWire<'a>,
}

#[derive(Serialize)]
struct FunctionDefinitionWire<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a Value,
    strict: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum ToolChoiceWire<'a> {
    Keyword(ToolChoiceKeywordWire),
    Specific(SpecificToolChoiceWire<'a>),
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ToolChoiceKeywordWire {
    None,
    Required,
}

#[derive(Serialize)]
pub(super) struct SpecificToolChoiceWire<'a> {
    #[serde(rename = "type")]
    kind: FunctionKindWire,
    function: SpecificFunctionWire<'a>,
}

#[derive(Serialize)]
pub(super) struct SpecificFunctionWire<'a> {
    name: &'a str,
}

pub(super) fn encode_tools<'a>(
    tools: &'a [ToolDefinition],
    capabilities: &CapabilitySet,
) -> Result<Option<Vec<ToolWire<'a>>>, LlmError> {
    if tools.is_empty() {
        return Ok(None);
    }
    require_capability("tools", "function_tools", capabilities.function_tools)?;
    let mut encoded = Vec::with_capacity(tools.len());
    for (index, tool) in tools.iter().enumerate() {
        let strict = if tool.strict() {
            require_capability(
                &format!("tools[{index}].strict"),
                "strict_tools",
                capabilities.strict_tools,
            )?;
            true
        } else {
            // Official P2 always emits an explicit false so goldens stay deterministic.
            false
        };
        encoded.push(ToolWire {
            kind: FunctionKindWire::Function,
            function: FunctionDefinitionWire {
                name: tool.name().as_str(),
                description: tool.description(),
                parameters: tool.parameters().value(),
                strict,
            },
        });
    }
    Ok(Some(encoded))
}

pub(super) fn encode_tool_choice<'a>(
    tools: &'a [ToolDefinition],
    choice: Option<&'a ToolChoice>,
    capabilities: &CapabilitySet,
) -> Result<Option<ToolChoiceWire<'a>>, LlmError> {
    let Some(choice) = choice else {
        return Ok(None);
    };
    match choice {
        ToolChoice::Auto => {
            if tools.is_empty() {
                return Err(
                    ProtocolError::new("auto tool choice requires at least one tool").into(),
                );
            }
            // Official default: omit auto when tools are present.
            Ok(None)
        }
        ToolChoice::None => Ok(Some(ToolChoiceWire::Keyword(ToolChoiceKeywordWire::None))),
        ToolChoice::Required => {
            require_capability(
                "tool_choice",
                "tool_choice_required",
                capabilities.tool_choice_required,
            )?;
            Ok(Some(ToolChoiceWire::Keyword(
                ToolChoiceKeywordWire::Required,
            )))
        }
        ToolChoice::Specific { name } => {
            require_capability(
                "tool_choice",
                "tool_choice_specific",
                capabilities.tool_choice_specific,
            )?;
            Ok(Some(ToolChoiceWire::Specific(SpecificToolChoiceWire {
                kind: FunctionKindWire::Function,
                function: SpecificFunctionWire {
                    name: name.as_str(),
                },
            })))
        }
    }
}

pub(super) fn encode_parallel_tool_calls(
    value: Option<ParallelToolCalls>,
    capabilities: &CapabilitySet,
) -> Result<Option<bool>, LlmError> {
    let Some(value) = value else {
        return Ok(None);
    };
    require_capability(
        "parallel_tool_calls",
        "parallel_tool_calls",
        capabilities.parallel_tool_calls,
    )?;
    Ok(Some(matches!(value, ParallelToolCalls::Enabled)))
}

fn require_capability(
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
