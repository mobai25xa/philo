//! Parallel tool-call handling: validate each call by id and preserve result order.

mod support;

use std::collections::BTreeMap;

use philo::domain::history::normalize_history;
use philo::domain::history::{DialectPolicy, HistoryCapabilities, HistoryPolicy};
use philo::domain::ids::{ToolCallId, ToolName};
use philo::domain::message::ToolResultMessage;
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::validate_tool_call;
use philo::domain::tools::{ParallelToolCalls, ToolArguments, ToolCall, ToolDefinition};
use philo::{ContentPart, Message, MessageRole};
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

fn clock_tool() -> ExampleResult<ToolDefinition> {
    let parameters = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "timezone": { "type": "string", "minLength": 1 }
        },
        "required": ["timezone"],
        "additionalProperties": false
    }))?;
    Ok(ToolDefinition::new(ToolName::new("get_time")?, parameters))
}

fn execute(call: &ToolCall) -> ExampleResult<String> {
    match call.name().as_str() {
        "get_weather" => {
            let city = call
                .arguments()
                .value()
                .get("city")
                .and_then(|value| value.as_str())
                .ok_or("missing city")?;
            Ok(format!(r#"{{"city":"{city}","temp_c":18}}"#))
        }
        "get_time" => {
            let timezone = call
                .arguments()
                .value()
                .get("timezone")
                .and_then(|value| value.as_str())
                .ok_or("missing timezone")?;
            Ok(format!(r#"{{"timezone":"{timezone}","now":"12:00"}}"#))
        }
        other => Err(format!("unknown tool in application table: {other}").into()),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let tools = [weather_tool()?, clock_tool()?];
    // Synthetic parallel turn; live networking is optional for this example.
    let calls = vec![
        ToolCall::new(
            ToolCallId::new("call_a")?,
            ToolName::new("get_weather")?,
            ToolArguments::from_raw_json(r#"{"city":"Paris"}"#)?,
        ),
        ToolCall::new(
            ToolCallId::new("call_b")?,
            ToolName::new("get_time")?,
            ToolArguments::from_raw_json(r#"{"timezone":"Europe/Paris"}"#)?,
        ),
    ];

    let mut results_by_id = BTreeMap::new();
    for call in &calls {
        match validate_tool_call(call.clone(), &tools) {
            Ok(validated) => {
                let result = if let Ok(payload) = execute(validated.call()) {
                    ToolResultMessage::text(
                        validated.call().id().clone(),
                        validated.call().name().clone(),
                        payload,
                    )?
                } else {
                    ToolResultMessage::error_text(
                        validated.call().id().clone(),
                        validated.call().name().clone(),
                        "tool execution failed",
                    )?
                };
                results_by_id.insert(validated.call().id().as_str().to_owned(), result);
            }
            Err(error) => {
                eprintln!("skipping invalid call: {error}");
                let result = ToolResultMessage::error_text(
                    call.id().clone(),
                    call.name().clone(),
                    "arguments failed validation",
                )?;
                results_by_id.insert(call.id().as_str().to_owned(), result);
            }
        }
    }

    // Preserve application-append order that still satisfies contiguous pairing.
    let mut history = vec![
        Message::user("Weather and local time for Paris?"),
        Message::new(
            MessageRole::Assistant,
            calls.into_iter().map(ContentPart::ToolCall).collect(),
        ),
    ];
    for result in results_by_id.into_values() {
        history.push(Message::from_tool_result(result));
    }

    let normalized = normalize_history(
        &history,
        &HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Unknown),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )?;
    println!(
        "parallel tool loop ok: {} messages; parallel preference available via {:?}",
        normalized.messages().len(),
        ParallelToolCalls::Enabled
    );
    Ok(())
}
