//! Provider-scoped routing merge, capability, wire, and fail-closed contracts.

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    ConstraintStrength, DataRetention, FallbackDimension, GenerateRequest, LlmClient, Message,
    ModelRef, OpenRouterRoutingContract, OpenRouterRoutingPatch, PolicySource,
    ProviderRequestOptions, RequestControl, RoutingFallback, RoutingField, RoutingRegion,
    RoutingSort, UpstreamId, ValidationReason,
};
use serde_json::json;

const ENDPOINT: &str = "http://127.0.0.1:41993/v1/chat/completions";
const ROUTING_FIXTURE: &str =
    include_str!("fixtures/provider-compat/openrouter/routing-request.json");

fn upstream(value: &str) -> UpstreamId {
    UpstreamId::new(value).unwrap()
}

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "routing-model").unwrap(),
        vec![Message::user("hello")],
    )
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

#[test]
fn allow_and_deny_conflicts_fail_before_transport() {
    let contract = OpenRouterRoutingContract::new(
        OpenRouterRoutingPatch::from_source(PolicySource::ProviderProfile)
            .with_allowed([upstream("alpha"), upstream("beta")]),
    );
    let request = OpenRouterRoutingPatch::from_source(PolicySource::Request)
        .with_allowed([upstream("beta")])
        .with_denied([upstream("beta")]);
    let error = contract.resolve(Some(&request)).unwrap_err();
    assert!(matches!(
        error,
        philo::LlmError::Validation(ref error)
            if error.reason() == ValidationReason::Conflict
    ));
}

#[test]
fn hard_region_and_retention_constraints_cannot_be_relaxed_by_fallback() {
    let contract = OpenRouterRoutingContract::new(
        OpenRouterRoutingPatch::from_source(PolicySource::ProviderProfile)
            .with_region(
                RoutingRegion::new("us-east").unwrap(),
                ConstraintStrength::Hard,
            )
            .with_data_retention(DataRetention::ZeroDataRetention, ConstraintStrength::Hard)
            .with_fallback(RoutingFallback::new(true, [FallbackDimension::Latency])),
    )
    .with_region_wire_support(true);
    let request = OpenRouterRoutingPatch::from_source(PolicySource::Request)
        .with_region(
            RoutingRegion::new("eu-west").unwrap(),
            ConstraintStrength::Preferred,
        )
        .with_data_retention(DataRetention::Allowed, ConstraintStrength::Preferred);
    assert!(contract.resolve(Some(&request)).is_err());
}

#[tokio::test]
async fn routing_is_only_encoded_for_declared_supported_profiles() {
    let mock = MockTransport::default();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "routing-key")
        .unwrap()
        .build()
        .unwrap();
    let control = RequestControl::new().with_provider_options(
        ProviderRequestOptions::new().with_openrouter_routing(
            OpenRouterRoutingPatch::from_source(PolicySource::Request)
                .with_sort(RoutingSort::Latency),
        ),
    );
    let error = LlmClient::new(runtime, mock.clone())
        .stream_with_control(request(), control)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        philo::LlmError::Validation(ref error)
            if error.reason() == ValidationReason::CapabilityUnsupported
    ));
    assert!(mock.captured_requests().is_empty());
}

#[test]
fn routing_merge_is_deterministic_and_source_traced() {
    let contract = OpenRouterRoutingContract::new(
        OpenRouterRoutingPatch::from_source(PolicySource::ProviderProfile)
            .with_allowed([upstream("alpha"), upstream("beta")])
            .with_sort(RoutingSort::Price),
    );
    let request = OpenRouterRoutingPatch::from_source(PolicySource::Request)
        .with_allowed([upstream("beta"), upstream("gamma")])
        .with_sort(RoutingSort::Throughput);
    let first = contract.resolve(Some(&request)).unwrap();
    let second = contract.resolve(Some(&request)).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first.source(RoutingField::AllowedUpstreams),
        Some(PolicySource::Request)
    );
    assert_eq!(
        first.source(RoutingField::Sort),
        Some(PolicySource::Request)
    );
}

#[tokio::test]
async fn private_encoder_emits_only_registered_fields() {
    let defaults = OpenRouterRoutingPatch::from_source(PolicySource::ProviderProfile)
        .with_allowed([upstream("alpha"), upstream("beta")])
        .with_denied([upstream("blocked")])
        .with_order([upstream("beta"), upstream("alpha")])
        .with_data_retention(DataRetention::ZeroDataRetention, ConstraintStrength::Hard)
        .with_fallback(RoutingFallback::new(false, []))
        .with_sort(RoutingSort::Latency);
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "routing-key")
        .unwrap()
        .with_openrouter_routing(OpenRouterRoutingContract::new(defaults))
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(success())]);
    LlmClient::new(runtime, mock.clone())
        .complete(request())
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(mock.captured_requests()[0].body()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(ROUTING_FIXTURE).unwrap();
    assert_eq!(body["provider"], expected);
    let keys = body["provider"].as_object().unwrap();
    assert_eq!(keys.len(), 7);
}

#[tokio::test]
async fn provider_scoped_routing_does_not_change_official_payloads() {
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "routing-key")
        .unwrap()
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(success())]);
    LlmClient::new(runtime, mock.clone())
        .complete(request())
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(mock.captured_requests()[0].body()).unwrap();
    assert!(body.get("provider").is_none());
    assert!(body.get("extra_body").is_none());
}
