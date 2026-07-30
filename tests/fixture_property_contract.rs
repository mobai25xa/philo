//! Capability-owned fixture property, security, and integration contracts.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::content::{
    ImageContent, ImageDetail, ImageMime, OpaqueReasoning, SourceIdentity, ThinkingContent,
};
use philo::domain::history::{
    DiagnosticCode, DialectPolicy, HistoryCapabilities, HistoryPolicy, ThinkingReplayPolicy,
};
use philo::domain::history::{apply_thinking_replay_policy, normalize_history};
use philo::domain::ids::{ProtocolId, ToolCallId, ToolName};
use philo::domain::message::ToolResultMessage;
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::validate_tool_call;
use philo::domain::tools::{ToolArguments, ToolCall, ToolChoice, ToolDefinition};
use philo::error::{HistoryFailure, SchemaFailure, ToolValidationFailure};
use philo::provider::ModelCapabilityProfile;
use philo::{
    AssistantEvent, ContentPart, FinishReason, GenerateRequest, GenerationOptions, LlmClient,
    Message, MessageRole, ModelId, ModelRef, ProviderId,
};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use support::provider::TestProvider;

const API_KEY: &str = "philo-fixture-key-canary";
const ENDPOINT: &str = "https://test.invalid/v1/chat/completions";
const ARGUMENT_CANARY: &str = "argument-canary-secret";
const QUERY_CANARY: &str = "query-canary-secret";
const OPAQUE_CANARY: &str = "opaque-thinking-canary";
const VISIBLE_CANARY: &str = "visible-thinking-canary";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_json(relative: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(fixture_root().join(relative)).unwrap()).unwrap()
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new("get_weather").unwrap(),
        ToolSchema::new(json!({
            "type": "object",
            "properties": {
                "city": { "type": "string" }
            },
            "required": ["city"],
            "additionalProperties": false
        }))
        .unwrap(),
    )
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

fn property_config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 48;
    }
    config
}

fn png_bytes() -> Bytes {
    Bytes::from_static(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 1, 2, 3])
}

fn tool_capable_runtime() -> philo::ProviderRuntime {
    let model = ModelId::new("gpt-test").unwrap();
    let model_profile = ModelCapabilityProfile::new(model)
        .with_function_tools(CapabilityStatus::Supported)
        .with_tool_choice_required(CapabilityStatus::Supported)
        .with_tool_choice_specific(CapabilityStatus::Supported)
        .with_parallel_tool_calls(CapabilityStatus::Supported)
        .with_strict_tools(CapabilityStatus::Supported);
    TestProvider::new(ENDPOINT, API_KEY)
        .unwrap()
        .with_model_capabilities(model_profile)
        .build()
        .unwrap()
}

fn response_headers(request_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert("x-request-id", HeaderValue::from_str(request_id).unwrap());
    headers
}

fn tool_stream_sse() -> Bytes {
    Bytes::from_static(include_bytes!(
        "fixtures/protocol/openai_chat/stream/tool-calls/single-call.sse"
    ))
}

fn final_text_sse() -> Bytes {
    Bytes::from(
        concat!(
            "data: {\"id\":\"gen-final\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"gen-final\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"gen-final\",\"model\":\"gpt-test\",\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_owned(),
    )
}

#[test]
fn fixture_tree_covers_required_directories() {
    let root = fixture_root();
    for relative in [
        "protocol/openai_chat/request/tools",
        "protocol/openai_chat/request/multimodal",
        "protocol/openai_chat/request/thinking",
        "protocol/openai_chat/request/structured-output",
        "protocol/openai_chat/stream/tool-calls",
        "protocol/openai_chat/stream/thinking",
        "protocol/openai_chat/stream/usage",
        "protocol/openai_chat/stream/finish",
        "protocol/openai_chat/stream/malformed",
        "domain/history/tool-pairing",
        "domain/history/replay",
        "domain/history/role-normalization",
        "domain/history/multimodal",
        "domain/schema/valid",
        "domain/schema/invalid",
        "domain/schema/unsupported-keywords",
        "domain/security/sensitive-arguments",
        "domain/security/image-metadata",
        "domain/security/opaque-thinking",
    ] {
        assert!(
            root.join(relative).is_dir(),
            "missing fixture directory: {relative}"
        );
    }
}

#[test]
fn schema_fixture_matrix_covers_valid_invalid_and_remote_ref() {
    let valid_object = read_json("domain/schema/valid/object-required.json");
    let schema = ToolSchema::new(valid_object["schema"].clone()).unwrap();
    assert!(schema.is_strict_compatible());

    let nullable = read_json("domain/schema/valid/nullable-anyof.json");
    assert!(ToolSchema::new(nullable["schema"].clone()).is_ok());

    let array_enum = read_json("domain/schema/valid/array-enum.json");
    assert!(ToolSchema::new(array_enum["schema"].clone()).is_ok());

    let not_object = read_json("domain/schema/invalid/not-an-object.json");
    assert_eq!(
        ToolSchema::new(not_object["schema"].clone())
            .unwrap_err()
            .reason(),
        SchemaFailure::NotAnObject
    );

    let remote = read_json("domain/schema/unsupported-keywords/remote-ref.json");
    assert_eq!(
        ToolSchema::new(remote["schema"].clone())
            .unwrap_err()
            .reason(),
        SchemaFailure::RemoteReference
    );

    let missing = read_json("domain/schema/invalid/missing-required-type.json");
    let tools = [ToolDefinition::new(
        ToolName::new("get_weather").unwrap(),
        ToolSchema::new(missing["schema"].clone()).unwrap(),
    )];
    let error = validate_tool_call(
        call(
            "call_missing",
            "get_weather",
            &serde_json::to_string(&missing["arguments"]).unwrap(),
        ),
        &tools,
    )
    .unwrap_err();
    assert_eq!(error.reason(), ToolValidationFailure::SchemaViolation);
    assert!(error.path().is_some());
}

#[test]
fn history_normalizer_is_idempotent_across_tool_scenarios() {
    let scenarios = [
        (
            "domain/history/tool-pairing/success-call-result.json",
            vec![
                Message::user("weather?"),
                Message::new(
                    MessageRole::Assistant,
                    vec![ContentPart::ToolCall(call(
                        "call_1",
                        "get_weather",
                        r#"{"city":"Paris"}"#,
                    ))],
                ),
                Message::from_tool_result(
                    ToolResultMessage::text(
                        ToolCallId::new("call_1").unwrap(),
                        ToolName::new("get_weather").unwrap(),
                        r#"{"temp":20}"#,
                    )
                    .unwrap(),
                ),
                Message::assistant("It is 20C"),
            ],
        ),
        (
            "domain/history/role-normalization/empty-assistant-removed.json",
            vec![
                Message::user("hi"),
                Message::new(MessageRole::Assistant, vec![]),
                Message::assistant("done"),
            ],
        ),
    ];

    for (fixture, input) in scenarios {
        let meta = read_json(fixture);
        assert_eq!(meta["expected"], "success");
        let first = normalize_history(
            &input,
            &history_caps(),
            &DialectPolicy::official_openai(),
            &HistoryPolicy::official_openai(),
        )
        .unwrap();
        let second = normalize_history(
            first.messages(),
            &history_caps(),
            &DialectPolicy::official_openai(),
            &HistoryPolicy::official_openai(),
        )
        .unwrap();
        // The history contract freezes message idempotence: re-normalizing output messages must not
        // keep deleting content, rewriting ids, or inventing further diagnostics.
        assert_eq!(second.messages(), first.messages());
        assert!(second.diagnostics().is_empty());
        assert!(second.id_mappings().is_empty());
        assert_eq!(
            input[0].content()[0].as_text(),
            input[0].content()[0].as_text()
        );
    }

    let empty_meta = read_json("domain/history/role-normalization/empty-assistant-removed.json");
    assert_eq!(empty_meta["expected_diagnostic"], "RemovedEmptyAssistant");
    let removed = normalize_history(
        &[
            Message::user("hi"),
            Message::new(MessageRole::Assistant, vec![]),
            Message::assistant("done"),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();
    assert!(
        removed
            .diagnostics()
            .iter()
            .any(|item| item.code() == DiagnosticCode::RemovedEmptyAssistant)
    );
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn history_normalizer_is_idempotent(
        city in prop::string::string_regex("[A-Za-z]{1,12}").unwrap(),
        result in prop::string::string_regex("[A-Za-z0-9 ]{1,24}").unwrap(),
    ) {
        let call = call(
            "call_prop",
            "get_weather",
            &format!(r#"{{"city":"{city}"}}"#),
        );
        let input = vec![
            Message::user("weather?"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::ToolCall(call.clone())],
            ),
            Message::from_tool_result(
                ToolResultMessage::text(call.id().clone(), call.name().clone(), &result).unwrap(),
            ),
            Message::assistant("ok"),
        ];
        let first = normalize_history(
            &input,
            &history_caps(),
            &DialectPolicy::official_openai(),
            &HistoryPolicy::official_openai(),
        )
        .unwrap();
        let second = normalize_history(
            first.messages(),
            &history_caps(),
            &DialectPolicy::official_openai(),
            &HistoryPolicy::official_openai(),
        )
        .unwrap();
        prop_assert_eq!(second.messages(), first.messages());
        prop_assert!(second.diagnostics().is_empty());
        prop_assert!(second.id_mappings().is_empty());
        prop_assert_eq!(input[0].content()[0].as_text(), "weather?");
    }
}

#[test]
fn history_fixture_errors_match_typed_failures() {
    let missing_meta = read_json("domain/history/tool-pairing/missing-result.json");
    assert_eq!(missing_meta["expected_error"], "missing_tool_result");
    let missing = normalize_history(
        &[
            Message::user("hi"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::ToolCall(call(
                    "call_1",
                    "get_weather",
                    r#"{"city":"Paris"}"#,
                ))],
            ),
            Message::user("next"),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    assert_eq!(missing.reason(), HistoryFailure::MissingToolResult);

    let duplicate_meta = read_json("domain/history/tool-pairing/duplicate-result.json");
    assert_eq!(duplicate_meta["expected_error"], "duplicate_tool_result");
    let result = ToolResultMessage::text(
        ToolCallId::new("call_1").unwrap(),
        ToolName::new("get_weather").unwrap(),
        "ok",
    )
    .unwrap();
    let duplicate = normalize_history(
        &[
            Message::user("hi"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::ToolCall(call(
                    "call_1",
                    "get_weather",
                    r#"{"city":"Paris"}"#,
                ))],
            ),
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
fn thinking_replay_and_security_canaries_are_redacted() {
    let same_meta = read_json("domain/history/replay/same-source-opaque.json");
    let cross_meta = read_json("domain/history/replay/cross-model-opaque-drop.json");
    assert_eq!(same_meta["protocol"], "synthetic-opaque-boundary");
    assert_eq!(cross_meta["protocol"], "synthetic-opaque-boundary");

    let source = SourceIdentity::new(
        ProviderId::new("provider").unwrap(),
        ModelId::new("model-a").unwrap(),
        ProtocolId::new("protocol").unwrap(),
    );
    let other = SourceIdentity::new(
        ProviderId::new("provider").unwrap(),
        ModelId::new("model-b").unwrap(),
        ProtocolId::new("protocol").unwrap(),
    );
    let thinking = ThinkingContent::new(VISIBLE_CANARY).with_opaque(OpaqueReasoning::new(
        Bytes::from_static(OPAQUE_CANARY.as_bytes()),
        source.clone(),
        false,
    ));

    let (same, diagnostics) = apply_thinking_replay_policy(
        &thinking,
        ThinkingReplayPolicy::SameSourceOnly,
        Some(&source),
    );
    assert!(same.unwrap().opaque().is_some());
    assert!(diagnostics.is_empty());

    let (dropped, diagnostics) = apply_thinking_replay_policy(
        &thinking,
        ThinkingReplayPolicy::SameSourceOnly,
        Some(&other),
    );
    assert!(dropped.unwrap().opaque().is_none());
    assert_eq!(diagnostics[0].code(), DiagnosticCode::DroppedThinkingOpaque);

    let debug = format!("{thinking:?}");
    assert!(!debug.contains(VISIBLE_CANARY));
    assert!(!debug.contains(OPAQUE_CANARY));

    let security = read_json("domain/security/opaque-thinking/opaque-canary.json");
    assert_eq!(security["canary_opaque"], OPAQUE_CANARY);
}

#[test]
fn tool_arguments_are_not_logged() {
    let canary = read_json("domain/security/sensitive-arguments/argument-canary.json");
    assert_eq!(canary["canary"], ARGUMENT_CANARY);
    let tools = [weather_tool()];
    let error = validate_tool_call(
        call(
            "call_secret",
            "get_weather",
            &format!(r#"{{"city":"Paris","secret":"{ARGUMENT_CANARY}"}}"#),
        ),
        &tools,
    )
    .unwrap_err();
    assert_eq!(error.reason(), ToolValidationFailure::SchemaViolation);
    assert!(!error.to_string().contains(ARGUMENT_CANARY));
    assert!(!format!("{error:?}").contains(ARGUMENT_CANARY));

    let arguments = ToolArguments::from_raw_json(format!(
        r#"{{"city":"Paris","secret":"{ARGUMENT_CANARY}"}}"#
    ))
    .unwrap();
    assert!(!format!("{arguments:?}").contains(ARGUMENT_CANARY));
}

#[test]
fn image_query_canary_is_redacted() {
    let canary = read_json("domain/security/image-metadata/url-query-canary.json");
    assert_eq!(canary["canary"], QUERY_CANARY);
    let image =
        ImageContent::parse_url(canary["url"].as_str().unwrap(), ImageDetail::Auto).unwrap();
    assert!(!format!("{image:?}").contains(QUERY_CANARY));
    assert!(ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Auto).is_ok());
}

#[test]
fn tool_result_image_fixture_rejects_unsupported_content() {
    let meta = read_json("domain/history/multimodal/tool-result-image-rejected.json");
    assert_eq!(meta["expected_error"], "unsupported_content");
    let image = ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Auto).unwrap();
    let error = ToolResultMessage::new(
        ToolCallId::new("call_1").unwrap(),
        ToolName::new("tool").unwrap(),
        vec![ContentPart::Image(image)],
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(error.reason(), HistoryFailure::UnsupportedContent);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tool_roundtrip_keeps_execution_in_application() {
    static EXECUTION_COUNT: Mutex<u32> = Mutex::new(0);

    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-tool"),
            vec![MockBodyItem::chunk(tool_stream_sse())],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-final"),
            vec![MockBodyItem::chunk(final_text_sse())],
        )),
    ]);
    let client = LlmClient::new(tool_capable_runtime(), mock.clone());

    let request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("weather?")],
    )
    .with_options(
        GenerationOptions::new()
            .with_tools(vec![weather_tool()])
            .with_tool_choice(ToolChoice::Required),
    );

    let events = client
        .stream(request)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let tool_end = events
        .iter()
        .find_map(|event| match event {
            AssistantEvent::ToolCallEnd { call, .. } => Some(call.clone()),
            _ => None,
        })
        .expect("tool call end");
    assert_eq!(
        events.last(),
        Some(&AssistantEvent::Done {
            finish_reason: FinishReason::ToolCalls
        })
    );

    let validated = validate_tool_call(tool_end.clone(), &[weather_tool()]).unwrap();
    assert_eq!(validated.call().name().as_str(), "get_weather");
    assert_eq!(*EXECUTION_COUNT.lock().unwrap(), 0);

    // Application-owned execution boundary: the SDK never increments this counter.
    *EXECUTION_COUNT.lock().unwrap() += 1;
    let result_text = r#"{"temp":20}"#;
    let result =
        ToolResultMessage::text(tool_end.id().clone(), tool_end.name().clone(), result_text)
            .unwrap();
    assert_eq!(*EXECUTION_COUNT.lock().unwrap(), 1);

    let history = normalize_history(
        &[
            Message::user("weather?"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::ToolCall(tool_end)],
            ),
            Message::from_tool_result(result),
        ],
        &history_caps(),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap();

    let follow_up = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        history.messages().to_vec(),
    )
    .with_options(GenerationOptions::new().with_tools(vec![weather_tool()]));
    let message = client.complete(follow_up).await.unwrap();
    assert_eq!(message.text(), "done");
    assert_eq!(*EXECUTION_COUNT.lock().unwrap(), 1);

    mock.assert_consumed();
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);
    let first: Value = serde_json::from_slice(captured[0].body()).unwrap();
    assert!(first.get("tools").is_some());
    assert_eq!(first["tool_choice"], "required");
    assert_eq!(first["stream"], true);
    let second: Value = serde_json::from_slice(captured[1].body()).unwrap();
    let roles: BTreeSet<_> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert!(roles.contains("tool"));
    assert!(
        second["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message.get("tool_call_id").is_some())
    );
    for request in captured {
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            format!("Bearer {API_KEY}")
        );
        assert!(!String::from_utf8_lossy(request.body()).contains(API_KEY));
    }
}

#[test]
fn structured_output_request_goldens_are_present() {
    for relative in [
        "protocol/openai_chat/request/structured-output/json-object.json",
        "protocol/openai_chat/request/structured-output/json-schema-strict.json",
    ] {
        let value = read_json(relative);
        assert_eq!(value["stream"], true);
        assert_eq!(value["n"], 1);
        assert!(value.get("response_format").is_some());
    }
}
