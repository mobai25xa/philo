//! Gateway routing is declared through the bounded body axis, not a first-class SDK type.
//!
//! `provider/compat/routing.rs` used to model one aggregation gateway's product
//! parameters as SDK types (FR-003). Those parameters are unknown top-level fields of
//! an `OpenAI` Chat request body, which is exactly what the bounded raw extension
//! already admits. This file is the migration evidence: the body the extension
//! produces is the body the retired encoder produced.

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::request::GenerationOptions;
use philo::error::ValidationReason;
use philo::protocol_options::{
    AnthropicMessagesOptions, OpenAiChatOptions, OpenAiChatRawExtension, ProtocolOptionDiagnostic,
};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{GenerateRequest, LlmClient, Message, ModelRef};
use serde_json::json;

const ENDPOINT: &str = "http://127.0.0.1:41993/v1/chat/completions";

/// The golden produced by the retired typed encoder. It is unchanged by FR-003:
/// the migration must reproduce it byte-for-byte in JSON value terms.
const LEGACY_ROUTING_FIXTURE: &str =
    include_str!("fixtures/provider-compat/openrouter/routing-request.json");

/// The exact bytes an `OpenAI` Chat request carries when no extension is used.
///
/// Retiring the typed `provider` field could not change these bytes — it was
/// `Option` + `skip_serializing_if`, so it never serialized for a caller that did
/// not opt in. This golden keeps that true: it turns red if the SDK-owned field
/// order or content ever shifts.
const NO_EXTENSION_BODY: &str = concat!(
    r#"{"model":"routing-model","messages":[{"role":"user","content":"hello"}],"#,
    r#""stream":true,"stream_options":{"include_usage":true},"n":1}"#
);

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "routing-model").unwrap(),
        vec![Message::user("hello")],
    )
}

/// The routing preferences the retired `OpenRouterRoutingPatch` builder expressed,
/// written directly as the gateway's documented wire object.
fn legacy_equivalent_routing() -> OpenAiChatRawExtension {
    OpenAiChatRawExtension::dangerous_from_object(json!({
        "provider": {
            "only": ["alpha", "beta"],
            "ignore": ["blocked"],
            "order": ["beta", "alpha"],
            "allow_fallbacks": false,
            "data_collection": "deny",
            "zdr": true,
            "sort": "latency"
        }
    }))
    .unwrap()
}

fn success() -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let delta = json!({
        "id": "routing-generation",
        "model": "routing-model",
        "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": null}]
    });
    let finish = json!({
        "id": "routing-generation",
        "model": "routing-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from(format!(
            "data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
        )))],
    )
}

async fn captured_body(request: GenerateRequest) -> Bytes {
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "routing-key")
        .unwrap()
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(success())]);
    LlmClient::new(runtime, mock.clone())
        .complete(request)
        .await
        .unwrap();
    mock.captured_requests()[0].body().clone()
}

#[tokio::test]
async fn migrated_routing_reproduces_the_retired_encoder_body() {
    let migrated = request().with_options(GenerationOptions::new().with_protocol_options(
        OpenAiChatOptions::new().with_raw_extension(legacy_equivalent_routing()),
    ));
    let body: serde_json::Value = serde_json::from_slice(&captured_body(migrated).await).unwrap();
    let legacy: serde_json::Value = serde_json::from_str(LEGACY_ROUTING_FIXTURE).unwrap();

    assert_eq!(body["provider"], legacy);
    assert_eq!(body["provider"].as_object().unwrap().len(), 7);
}

#[tokio::test]
async fn each_retired_routing_dimension_survives_the_migration() {
    // sort / upstream allow-deny-order / data-retention / fallback: the four families
    // the retired `OpenRouterRoutingPatch` could express.
    for (dimension, value) in [
        ("sort", json!({"sort": "throughput"})),
        (
            "upstreams",
            json!({"only": ["alpha"], "ignore": ["blocked"], "order": ["alpha"]}),
        ),
        (
            "data_retention",
            json!({"data_collection": "deny", "zdr": true}),
        ),
        ("fallback", json!({"allow_fallbacks": false})),
    ] {
        let raw =
            OpenAiChatRawExtension::dangerous_from_object(json!({ "provider": value.clone() }))
                .unwrap();
        let migrated = request().with_options(
            GenerationOptions::new()
                .with_protocol_options(OpenAiChatOptions::new().with_raw_extension(raw)),
        );
        let body: serde_json::Value =
            serde_json::from_slice(&captured_body(migrated).await).unwrap();
        assert_eq!(body["provider"], value, "dimension lost: {dimension}");
    }
}

#[tokio::test]
async fn a_request_without_the_extension_is_byte_identical_to_the_pre_migration_body() {
    let body = captured_body(request()).await;
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), NO_EXTENSION_BODY);

    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(value.get("provider").is_none());
    assert!(value.get("extra_body").is_none());
}

#[test]
fn the_body_axis_still_refuses_sdk_owned_fields_and_credential_shapes() {
    for value in [
        json!({"model": "canary-secret-value"}),
        json!({"messages": "canary-secret-value"}),
        json!({"stream": false}),
        json!({"tools": []}),
        json!({"response_format": {"type": "json_object"}}),
        json!({"authorization": "canary-secret-value"}),
        json!({"x-api-key": "canary-secret-value"}),
    ] {
        let error = OpenAiChatRawExtension::dangerous_from_object(value).unwrap_err();
        assert_eq!(error.reason(), ValidationReason::Conflict);
        assert!(!error.to_string().contains("canary-secret-value"));
    }
}

#[test]
fn using_the_body_axis_is_reported_as_non_portable_without_leaking_values() {
    let raw = legacy_equivalent_routing();
    assert_eq!(
        raw.diagnostic(),
        ProtocolOptionDiagnostic::NonPortableExtensionUsed
    );
    let options = OpenAiChatOptions::new().with_raw_extension(raw);
    assert_eq!(
        options.diagnostics(),
        vec![ProtocolOptionDiagnostic::NonPortableExtensionUsed]
    );
    assert!(!format!("{options:?}").contains("alpha"));
}

#[tokio::test]
async fn options_for_another_protocol_fail_before_transport() {
    let mock = MockTransport::default();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "routing-key")
        .unwrap()
        .build()
        .unwrap();
    let mismatched = request().with_options(
        GenerationOptions::new().with_protocol_options(AnthropicMessagesOptions::new()),
    );
    let error = LlmClient::new(runtime, mock.clone())
        .stream(mismatched)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        philo::LlmError::Validation(ref error)
            if error.reason() == ValidationReason::Conflict
    ));
    assert!(mock.captured_requests().is_empty());
}
