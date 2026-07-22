//! R2-A07 raw domain-boundary regression contracts.

use std::fs;
use std::path::{Path, PathBuf};

use philo::{
    CapabilityStatus, ContentPart, DialectPolicy, HistoryCapabilities, HistoryFailure,
    HistoryPolicy, ImageContent, ImageDetail, Message, MessageRole, SchemaFailure, SchemaLimits,
    ToolArguments, ToolCall, ToolCallId, ToolName, ToolResultMessage, ToolSchema, ValidationReason,
    normalize_history,
};
use serde::Deserialize;
use serde_json::Value;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase-2/repair")
}

fn json_fixture(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(fixture_root().join(path)).unwrap()).unwrap()
}

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall::new(
        ToolCallId::new(id).unwrap(),
        ToolName::new(name).unwrap(),
        ToolArguments::from_raw_json("{}").unwrap(),
    )
}

fn result(id: &str, name: &str) -> Message {
    Message::from_tool_result(
        ToolResultMessage::text(
            ToolCallId::new(id).unwrap(),
            ToolName::new(name).unwrap(),
            "ok",
        )
        .unwrap(),
    )
}

fn normalize(messages: &[Message]) -> Result<philo::NormalizedContext, philo::HistoryError> {
    normalize_history(
        messages,
        &HistoryCapabilities::new(CapabilityStatus::Supported, CapabilityStatus::Supported),
        &DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
}

#[test]
fn history_ids_are_unique_per_assistant_turn_and_normalization_is_idempotent() {
    let fixture = json_fixture("planner/repeated-id-across-turns.json");
    let id = fixture["tool_call_id"].as_str().unwrap();
    let name = fixture["tool_name"].as_str().unwrap();
    let messages = vec![
        Message::user("first"),
        Message::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall(call(id, name))],
        ),
        result(id, name),
        Message::user("second"),
        Message::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall(call(id, name))],
        ),
        result(id, name),
    ];
    let first = normalize(&messages).unwrap();
    let second = normalize(first.messages()).unwrap();
    assert_eq!(first.messages(), second.messages());
    assert_eq!(first.id_mappings(), second.id_mappings());
}

#[test]
fn missing_result_and_normalized_id_fail_or_succeed_at_the_history_boundary() {
    let missing = json_fixture("planner/missing-tool-result.json");
    let id = missing["tool_call_id"].as_str().unwrap();
    let name = missing["tool_name"].as_str().unwrap();
    let error = normalize(&[
        Message::user("lookup"),
        Message::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall(call(id, name))],
        ),
    ])
    .unwrap_err();
    assert_eq!(error.reason(), HistoryFailure::MissingToolResult);

    let normalized = json_fixture("planner/normalized-tool-id.json");
    let original = normalized["original_id"].as_str().unwrap();
    let name = normalized["tool_name"].as_str().unwrap();
    let context = normalize(&[
        Message::user("lookup"),
        Message::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall(call(original, name))],
        ),
        result(original, name),
    ])
    .unwrap();
    let mapping = context.id_mappings().first().unwrap();
    assert_eq!(mapping.original().as_str(), original);
    assert!(mapping.normalized().as_str().len() <= 40);
    assert!(
        mapping
            .normalized()
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    );
}

#[test]
fn schema_reference_cycles_and_depth_are_bounded() {
    for fixture in ["schema/self-ref.json", "schema/mutual-ref.json"] {
        let error = ToolSchema::new(json_fixture(fixture)).unwrap_err();
        assert_eq!(error.reason(), SchemaFailure::TooDeep);
    }

    let limits = SchemaLimits {
        max_schema_bytes: 256 * 1024,
        max_schema_depth: 6,
        max_json_array_items: 65_536,
    };
    let error =
        ToolSchema::with_limits(json_fixture("schema/reference-depth-overflow.json"), limits)
            .unwrap_err();
    assert_eq!(error.reason(), SchemaFailure::TooDeep);
}

#[test]
fn schema_pointer_escaping_and_numeric_keywords_are_strict() {
    let schema = ToolSchema::new(json_fixture("schema/escaped-pointer.json")).unwrap();
    schema
        .validate_instance(&serde_json::json!({ "ok": true }), SchemaLimits::official())
        .unwrap();

    for fixture in [
        "schema/fractional-max-items.json",
        "schema/negative-min-length.json",
    ] {
        let error = ToolSchema::new(json_fixture(fixture)).unwrap_err();
        assert_eq!(error.reason(), SchemaFailure::InvalidKeywordType);
    }
    let illegal_escape = ToolSchema::new(serde_json::json!({
        "$ref": "#/$defs/bad~2name",
        "$defs": { "bad~2name": { "type": "string" } }
    }))
    .unwrap_err();
    assert_eq!(illegal_escape.reason(), SchemaFailure::InvalidKeywordType);
    let inverted = ToolSchema::new(serde_json::json!({
        "type": "number",
        "minimum": 2,
        "maximum": 1
    }))
    .unwrap_err();
    assert_eq!(inverted.reason(), SchemaFailure::InvalidKeywordType);
}

#[derive(Deserialize)]
struct ImageSecurityFixture {
    url: String,
    expected_error: Option<String>,
    canary: Option<String>,
}

fn image_fixture(path: &str) -> ImageSecurityFixture {
    toml::from_str(&fs::read_to_string(fixture_root().join(path)).unwrap()).unwrap()
}

#[test]
fn image_url_userinfo_is_rejected_without_retaining_the_url() {
    let fixture = image_fixture("security/image-userinfo.toml");
    assert_eq!(
        fixture.expected_error.as_deref(),
        Some("invalid_identifier")
    );
    let error = ImageContent::parse_url(&fixture.url, ImageDetail::Auto).unwrap_err();
    assert_eq!(error.reason(), ValidationReason::InvalidIdentifier);
    assert!(!format!("{error:?}").contains(&fixture.url));
    assert!(!error.to_string().contains(&fixture.url));

    let redaction = image_fixture("security/redaction-canary.toml");
    let canary = redaction.canary.unwrap();
    let error = ImageContent::parse_url(&redaction.url, ImageDetail::Auto).unwrap_err();
    assert!(!format!("{error:?}").contains(&canary));
    assert!(!error.to_string().contains(&canary));
}

#[test]
fn tool_example_uses_json_serialization_for_model_controlled_text() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tool_single.rs"))
            .unwrap();
    assert!(source.contains("json!({"));
    assert!(!source.contains(r#"r#\"{{\"city\":\"{city}"#));
}
