//! Distinguishes SDK validation failures from application permission / execution denials.

mod support;

use philo::domain::ids::{ToolCallId, ToolName};
use philo::domain::message::ToolResultMessage;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::validate_tool_call;
use philo::domain::tools::{ToolArguments, ToolCall, ToolDefinition};
use philo::error::ToolValidationFailure;
use serde_json::json;
use support::ExampleResult;

fn weather_tool() -> ExampleResult<ToolDefinition> {
    let parameters = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "city": { "type": "string", "minLength": 1 }
        },
        "required": ["city"],
        "additionalProperties": false
    }))?;
    Ok(ToolDefinition::new(
        ToolName::new("get_weather")?,
        parameters,
    ))
}

#[derive(Debug)]
enum AppToolError {
    Validation,
    PermissionDenied(&'static str),
    Execution,
}

fn application_permission(call: &ToolCall) -> Result<(), AppToolError> {
    // Example-only application policy. Not part of LlmError.
    if call
        .arguments()
        .value()
        .get("city")
        .and_then(|value| value.as_str())
        == Some("blocked-city")
    {
        return Err(AppToolError::PermissionDenied(
            "city is not allowed for this tenant",
        ));
    }
    Ok(())
}

fn handle(call: &ToolCall, tools: &[ToolDefinition]) -> ExampleResult {
    match validate_tool_call(call.clone(), tools) {
        Ok(validated) => match application_permission(validated.call()) {
            Ok(()) => {
                // Execution is still application owned. Pretend the external system failed.
                let app_error = AppToolError::Execution;
                let result = ToolResultMessage::error_text(
                    validated.call().id().clone(),
                    validated.call().name().clone(),
                    "upstream weather service unavailable",
                )?;
                println!(
                    "execution failed safely ({app_error:?}): is_error={}",
                    result.is_error()
                );
                Ok(())
            }
            Err(AppToolError::PermissionDenied(reason)) => {
                let result = ToolResultMessage::error_text(
                    validated.call().id().clone(),
                    validated.call().name().clone(),
                    reason,
                )?;
                println!(
                    "application denied execution; returning error result for {}",
                    result.tool_call_id().as_str()
                );
                Ok(())
            }
            Err(other) => Err(format!("unexpected application error: {other:?}").into()),
        },
        Err(error) => {
            assert_eq!(error.reason(), ToolValidationFailure::SchemaViolation);
            let app_error = AppToolError::Validation;
            println!("sdk validation failed without executing ({app_error:?}): {error}");
            // No ToolResult is required when the application aborts the turn before execution.
            Ok(())
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let tools = [weather_tool()?];

    let invalid = ToolCall::new(
        ToolCallId::new("call_invalid")?,
        ToolName::new("get_weather")?,
        ToolArguments::from_raw_json(r#"{"units":"c"}"#)?,
    );
    handle(&invalid, &tools)?;

    let denied = ToolCall::new(
        ToolCallId::new("call_denied")?,
        ToolName::new("get_weather")?,
        ToolArguments::from_raw_json(r#"{"city":"blocked-city"}"#)?,
    );
    handle(&denied, &tools)?;

    Ok(())
}
