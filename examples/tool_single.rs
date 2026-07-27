//! Single-tool loop: declare → stream/complete tool call → validate → app execute → result.
//!
//! Without `OPENAI_API_KEY` / `OPENAI_MODEL`, the example only exercises offline validation and
//! history pairing so it remains a safe compile and local demo.

mod support;

use philo::domain::history::normalize_history;
use philo::domain::history::{DialectPolicy, HistoryCapabilities, HistoryPolicy};
use philo::domain::ids::{ToolCallId, ToolName};
use philo::domain::message::ToolResultMessage;
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::validate_tool_call;
use philo::domain::tools::{ToolArguments, ToolCall, ToolChoice, ToolDefinition};
use philo::{ContentPart, GenerationOptions, Message, MessageRole};
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
    Ok(
        ToolDefinition::new(ToolName::new("get_weather")?, parameters)
            .with_description("Return a synthetic weather summary")?,
    )
}

/// Application-owned execution. The SDK never calls this automatically.
fn execute_weather(call: &ToolCall) -> ExampleResult<String> {
    let city = call
        .arguments()
        .value()
        .get("city")
        .and_then(|value| value.as_str())
        .ok_or("validated call missing city")?;
    // Application policy belongs here: permission checks, allow-lists, redaction.
    Ok(json!({
        "city": city,
        "temp_c": 20,
        "source": "synthetic",
    })
    .to_string())
}

fn offline_demo(tool: ToolDefinition) -> ExampleResult {
    let call = ToolCall::new(
        ToolCallId::new("call_weather_1")?,
        tool.name().clone(),
        ToolArguments::from_raw_json(r#"{"city":"Paris"}"#)?,
    );
    let validated = validate_tool_call(call.clone(), &[tool])?;
    let payload = execute_weather(validated.call())?;
    let result = ToolResultMessage::text(
        validated.call().id().clone(),
        validated.call().name().clone(),
        payload,
    )?;
    let history = vec![
        Message::user("What is the weather in Paris?"),
        Message::new(MessageRole::Assistant, vec![ContentPart::ToolCall(call)]),
        Message::from_tool_result(result),
    ];
    let normalized = normalize_history(
        &history,
        &HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Unknown),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )?;
    println!(
        "offline tool loop ok: {} messages after normalize",
        normalized.messages().len()
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let tool = weather_tool()?;
    if !support::has_live_credentials() {
        return offline_demo(tool);
    }

    let client = support::client_with_phase2_capabilities()?;
    let options = GenerationOptions::new()
        .with_tools(vec![tool.clone()])
        .with_tool_choice(ToolChoice::Required);
    let request = support::request("Call get_weather for Paris and wait.")?.with_options(options);

    let message = client.complete(request).await?;
    let mut next_messages = vec![
        Message::user("Call get_weather for Paris and wait."),
        Message::new(MessageRole::Assistant, message.content().to_vec()),
    ];

    for part in message.content() {
        let ContentPart::ToolCall(call) = part else {
            continue;
        };
        match validate_tool_call(call.clone(), std::slice::from_ref(&tool)) {
            Ok(validated) => {
                let payload = execute_weather(validated.call())?;
                let result = ToolResultMessage::text(
                    validated.call().id().clone(),
                    validated.call().name().clone(),
                    payload,
                )?;
                next_messages.push(Message::from_tool_result(result));
            }
            Err(error) => {
                // Validation failure is not tool execution. Return a safe error result only when
                // the application still wants the model to continue.
                eprintln!("validation rejected tool call: {error}");
                let result = ToolResultMessage::error_text(
                    call.id().clone(),
                    call.name().clone(),
                    "tool arguments failed schema validation",
                )?;
                next_messages.push(Message::from_tool_result(result));
            }
        }
    }

    let follow_up = philo::GenerateRequest::new(
        philo::ModelRef::new("official-openai", std::env::var("OPENAI_MODEL")?)?,
        next_messages,
    )
    .with_options(GenerationOptions::new().with_tools(vec![tool]));
    let final_message = client.complete(follow_up).await?;
    println!("{}", final_message.text());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_result_escapes_model_controlled_city_text() {
        let call = ToolCall::new(
            ToolCallId::new("call_escape").unwrap(),
            ToolName::new("get_weather").unwrap(),
            ToolArguments::from_raw_json(r#"{"city":"quote\" slash\\ newline\n"}"#).unwrap(),
        );
        let payload = execute_weather(&call).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["city"], "quote\" slash\\ newline\n");
    }
}
