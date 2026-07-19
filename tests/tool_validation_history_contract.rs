//! Phase-two tool validation, tool result pairing, and history normalization tests.

use philo::{
    CapabilityStatus, ContentPart, DiagnosticCode, DialectPolicy, HistoryCapabilities,
    HistoryFailure, HistoryPolicy, LlmError, Message, MissingToolResultPolicy, ThinkingContent,
    ThinkingReplayPolicy, ToolArguments, ToolCall, ToolCallId, ToolDefinition, ToolName,
    ToolResultMessage, ToolSchema, ToolValidationFailure, UnsupportedContentPolicy,
    normalize_history, validate_tool_call,
};
use serde_json::json;

fn object_schema() -> ToolSchema {
    ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "city": { "type": "string", "minLength": 1 },
            "units": { "type": "string", "enum": ["c", "f"] },
            "days": { "type": "integer", "minimum": 1, "maximum": 7 }
        },
        "required": ["city"],
        "additionalProperties": false
    }))
    .unwrap()
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition::new(ToolName::new("get_weather").unwrap(), object_schema())
}

fn call(id: &str, name: &str, raw: &str) -> ToolCall {
    ToolCall::new(
        ToolCallId::new(id).unwrap(),
        ToolName::new(name).unwrap(),
        ToolArguments::from_raw_json(raw).unwrap(),
    )
}

fn history_caps() -> HistoryCapabilities {
    HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Unknown)
}

#[test]
fn validate_tool_call_accepts_schema_and_rejects_unknown_or_invalid_fields() {
    let tools = [weather_tool()];
    let ok = validate_tool_call(
        call("call_1", "get_weather", r#"{"city":"Paris","units":"c"}"#),
        &tools,
    )
    .unwrap();
    assert_eq!(ok.call().name().as_str(), "get_weather");
    assert_eq!(ok.into_call().arguments().value()["city"], "Paris");

    let unknown = validate_tool_call(
        call("call_2", "missing_tool", r#"{"city":"Paris"}"#),
        &tools,
    )
    .unwrap_err();
    assert_eq!(unknown.reason(), ToolValidationFailure::UnknownTool);
    assert!(!unknown.to_string().contains("Paris"));

    let missing =
        validate_tool_call(call("call_3", "get_weather", r#"{"units":"c"}"#), &tools).unwrap_err();
    assert_eq!(missing.reason(), ToolValidationFailure::SchemaViolation);
    assert!(missing.path().is_some());

    let extra = validate_tool_call(
        call(
            "call_4",
            "get_weather",
            r#"{"city":"Paris","secret":"argument-canary"}"#,
        ),
        &tools,
    )
    .unwrap_err();
    assert_eq!(extra.reason(), ToolValidationFailure::SchemaViolation);
    assert!(!extra.to_string().contains("argument-canary"));
    assert!(!format!("{extra:?}").contains("argument-canary"));
}

#[test]
fn validate_tool_call_enforces_array_and_depth_limits() {
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["items"],
        "additionalProperties": false
    }))
    .unwrap();
    let tools = [ToolDefinition::new(ToolName::new("bulk").unwrap(), schema)];

    let too_many = json!({
        "items": vec!["x"; 70_000]
    })
    .to_string();
    let error = validate_tool_call(call("call_bulk", "bulk", &too_many), &tools).unwrap_err();
    assert!(matches!(
        error.reason(),
        ToolValidationFailure::ArgumentsTooLarge | ToolValidationFailure::SchemaViolation
    ));
    assert!(!error.to_string().contains("items"));
}

#[test]
fn tool_result_message_requires_single_non_empty_text() {
    let id = ToolCallId::new("call_1").unwrap();
    let name = ToolName::new("get_weather").unwrap();
    let ok = ToolResultMessage::text(id.clone(), name.clone(), "sunny").unwrap();
    assert!(!ok.is_error());
    assert_eq!(ok.content()[0].as_text(), "sunny");

    let err = ToolResultMessage::error_text(id.clone(), name.clone(), "lookup failed").unwrap();
    assert!(err.is_error());

    let empty = ToolResultMessage::text(id.clone(), name.clone(), "").unwrap_err();
    assert_eq!(empty.reason(), HistoryFailure::UnsupportedContent);

    let thinking = ToolResultMessage::new(
        id,
        name,
        vec![ContentPart::Thinking(ThinkingContent::new("nope"))],
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(thinking.reason(), HistoryFailure::UnsupportedContent);
}

#[test]
fn history_pairs_tool_calls_and_results_and_is_idempotent() {
    let call = call("call_1", "get_weather", r#"{"city":"Paris"}"#);
    let result =
        ToolResultMessage::text(call.id().clone(), call.name().clone(), r#"{"temp":20}"#).unwrap();
    let input = vec![
        Message::user("weather?"),
        Message::new(
            philo::MessageRole::Assistant,
            vec![ContentPart::ToolCall(call)],
        ),
        Message::from_tool_result(result),
        Message::assistant("It is 20C"),
    ];

    let first = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();
    assert_eq!(first.messages().len(), 4);
    assert!(first.messages()[2].tool_result().is_some());

    let second = normalize_history(
        first.messages(),
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();
    assert_eq!(second.messages(), first.messages());
    assert_eq!(second.diagnostics(), first.diagnostics());
    assert_eq!(input[0].content()[0].as_text(), "weather?");
}

#[test]
fn history_rejects_missing_unknown_and_duplicate_results() {
    let call = call("call_1", "get_weather", r#"{"city":"Paris"}"#);
    let assistant = Message::new(
        philo::MessageRole::Assistant,
        vec![ContentPart::ToolCall(call.clone())],
    );

    let missing = normalize_history(
        &[
            Message::user("hi"),
            assistant.clone(),
            Message::user("next"),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    assert_eq!(missing.reason(), HistoryFailure::MissingToolResult);

    let unknown = ToolResultMessage::text(
        ToolCallId::new("other").unwrap(),
        call.name().clone(),
        "nope",
    )
    .unwrap();
    let unknown_err = normalize_history(
        &[
            Message::user("hi"),
            assistant.clone(),
            Message::from_tool_result(unknown),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    assert_eq!(unknown_err.reason(), HistoryFailure::UnknownToolCall);

    let result = ToolResultMessage::text(call.id().clone(), call.name().clone(), "ok").unwrap();
    let duplicate = normalize_history(
        &[
            Message::user("hi"),
            assistant,
            Message::from_tool_result(result.clone()),
            Message::from_tool_result(result),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    assert_eq!(duplicate.reason(), HistoryFailure::DuplicateToolResult);
}

#[test]
fn history_sanitizes_tool_call_ids_and_drops_thinking() {
    let call = call("call/1@raw", "get_weather", r#"{"city":"Paris"}"#);
    let result = ToolResultMessage::text(call.id().clone(), call.name().clone(), "ok").unwrap();
    let input = vec![
        Message::user("hi"),
        Message::new(
            philo::MessageRole::Assistant,
            vec![
                ContentPart::Thinking(ThinkingContent::new("secret-thinking")),
                ContentPart::ToolCall(call),
            ],
        ),
        Message::from_tool_result(result),
    ];

    let normalized = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();

    let assistant = &normalized.messages()[1];
    assert!(
        assistant
            .content()
            .iter()
            .all(|part| !matches!(part, ContentPart::Thinking(_)))
    );
    let ContentPart::ToolCall(normalized_call) = &assistant.content()[0] else {
        panic!("expected tool call");
    };
    assert!(
        normalized_call
            .id()
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    );
    assert!(normalized_call.id().as_str().len() <= 40);
    assert!(!normalized.id_mappings().is_empty());
    assert!(
        normalized
            .diagnostics()
            .iter()
            .any(|item| item.code() == DiagnosticCode::DroppedThinkingOpaque
                || item.code() == DiagnosticCode::SanitizedToolCallId)
    );

    let tool_message = &normalized.messages()[2];
    assert_eq!(
        tool_message.tool_result().unwrap().tool_call_id().as_str(),
        normalized_call.id().as_str()
    );
    assert!(!format!("{normalized:?}").contains("secret-thinking"));
}

#[test]
fn history_removes_empty_assistant_and_rejects_unsupported_policy() {
    let input = vec![
        Message::user("hi"),
        Message::new(philo::MessageRole::Assistant, vec![]),
        Message::assistant("done"),
    ];
    let normalized = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();
    assert_eq!(normalized.messages().len(), 2);
    assert!(
        normalized
            .diagnostics()
            .iter()
            .any(|item| item.code() == DiagnosticCode::RemovedEmptyAssistant)
    );

    let mut policy = HistoryPolicy::official_openai();
    policy.missing_tool_result = MissingToolResultPolicy::SynthesizeError;
    let error = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &policy,
    )
    .unwrap_err();
    assert_eq!(error.reason(), HistoryFailure::UnsupportedPolicy);

    policy = HistoryPolicy::official_openai();
    policy.thinking_replay = ThinkingReplayPolicy::SameSourceOnly;
    let error = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &policy,
    )
    .unwrap_err();
    assert_eq!(error.reason(), HistoryFailure::UnsupportedPolicy);

    policy = HistoryPolicy::official_openai();
    policy.unsupported_content = UnsupportedContentPolicy::DropWithDiagnostic;
    let error = normalize_history(
        &input,
        &history_caps(),
        &DialectPolicy::official_openai(),
        &policy,
    )
    .unwrap_err();
    assert_eq!(error.reason(), HistoryFailure::UnsupportedPolicy);
}

#[test]
fn history_errors_surface_as_typed_llm_error_without_content() {
    let call = call("call_1", "get_weather", r#"{"city":"secret-city"}"#);
    let error = normalize_history(
        &[
            Message::user("hi"),
            Message::new(
                philo::MessageRole::Assistant,
                vec![ContentPart::ToolCall(call)],
            ),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    let llm = LlmError::from(error);
    assert!(matches!(llm, LlmError::History(_)));
    assert!(!llm.to_string().contains("secret-city"));
    assert!(!format!("{llm:?}").contains("secret-city"));
}
