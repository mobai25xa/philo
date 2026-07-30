//! Real-provider preset, catalog, compatibility, and wire contracts.

mod support;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::request::CapabilityStatus;
use philo::{GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef};
use philo_presets::{
    DeepSeekProfile, OpenRouterAttribution, OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile,
};
use serde_json::json;
use support::mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};

const CANARY: &str = "real-provider-credential-canary";

#[test]
fn presets_freeze_exact_product_endpoint_catalog_and_experimental_support() {
    let runtimes = [
        OpenRouterProfile::from_api_key(CANARY)
            .unwrap()
            .build()
            .unwrap(),
        DeepSeekProfile::from_api_key(CANARY)
            .unwrap()
            .build()
            .unwrap(),
        ZaiStandardProfile::from_api_key(CANARY)
            .unwrap()
            .build()
            .unwrap(),
        ZaiCodingProfile::from_api_key(CANARY)
            .unwrap()
            .build()
            .unwrap(),
    ];
    let expected = [
        (
            "openrouter",
            "openrouter-chat",
            "nvidia/nemotron-3-ultra-550b-a55b:free",
            "https://openrouter.ai/api/v1/chat/completions",
        ),
        (
            "deepseek",
            "deepseek-chat-openai",
            "deepseek-v4-flash",
            "https://api.deepseek.com/chat/completions",
        ),
        (
            "zai",
            "zai-standard-api",
            "glm-4.7-flash",
            "https://api.z.ai/api/paas/v4/chat/completions",
        ),
        (
            "zai",
            "zai-coding-plan",
            "glm-4.7-flash",
            "https://api.z.ai/api/coding/paas/v4/chat/completions",
        ),
    ];
    for (runtime, (provider, product, model, endpoint)) in runtimes.iter().zip(expected) {
        assert_eq!(runtime.provider_id().as_str(), provider);
        assert_eq!(runtime.product_id().as_str(), product);
        assert_eq!(runtime.endpoint().url().as_str(), endpoint);
        let entry = runtime
            .catalog()
            .entries()
            .find(|entry| entry.key.domain_model_id.as_str() == model)
            .unwrap();
        assert_eq!(entry.support_status, CapabilityStatus::Supported);
        assert_eq!(entry.wire_model_value.as_str(), model);
        assert!(!format!("{runtime:?}").contains(CANARY));
    }
}

#[test]
fn structured_provider_identity_headers_are_allowlisted_and_redacted() {
    let attribution = OpenRouterAttribution::new("https://philo.example", "philo app")
        .unwrap()
        .with_categories(["sdk", "rust"])
        .unwrap();
    assert!(!format!("{attribution:?}").contains("philo.example"));
    let openrouter = OpenRouterProfile::from_api_key(CANARY)
        .unwrap()
        .with_attribution(attribution)
        .build()
        .unwrap();
    let headers = openrouter
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    assert_eq!(headers.headers()["http-referer"], "https://philo.example");
    assert_eq!(headers.headers()["x-openrouter-title"], "philo app");
    assert_eq!(headers.headers()["x-openrouter-categories"], "sdk,rust");

    let zai = ZaiStandardProfile::from_api_key(CANARY)
        .unwrap()
        .with_accept_language("zh-CN")
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        zai.resolve_headers(Vec::new(), &HeaderMap::new())
            .unwrap()
            .headers()["accept-language"],
        "zh-CN"
    );
    assert!(OpenRouterAttribution::new("https://example.com/path", "app").is_err());
    assert!(
        ZaiStandardProfile::from_api_key(CANARY)
            .unwrap()
            .with_accept_language("zh-CN, secret")
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_provider_runtimes_do_not_cross_talk_headers_or_credentials() {
    let runtimes = [
        (
            OpenRouterProfile::from_api_key("openrouter-isolated")
                .unwrap()
                .with_attribution(OpenRouterAttribution::new("https://one.example", "one").unwrap())
                .build()
                .unwrap(),
            "openrouter-isolated",
            Some(("http-referer", "https://one.example")),
        ),
        (
            DeepSeekProfile::from_api_key("deepseek-isolated")
                .unwrap()
                .build()
                .unwrap(),
            "deepseek-isolated",
            None,
        ),
        (
            ZaiStandardProfile::from_api_key("zai-standard-isolated")
                .unwrap()
                .with_accept_language("zh-CN")
                .unwrap()
                .build()
                .unwrap(),
            "zai-standard-isolated",
            Some(("accept-language", "zh-CN")),
        ),
        (
            ZaiCodingProfile::from_api_key("zai-coding-isolated")
                .unwrap()
                .with_accept_language("en-US")
                .unwrap()
                .build()
                .unwrap(),
            "zai-coding-isolated",
            Some(("accept-language", "en-US")),
        ),
    ];
    let tasks = runtimes.into_iter().map(|(runtime, secret, extra)| {
        tokio::spawn(async move {
            for _ in 0..16 {
                let resolved = runtime
                    .resolve_headers(Vec::new(), &HeaderMap::new())
                    .unwrap();
                assert_eq!(
                    resolved.headers()[header::AUTHORIZATION],
                    format!("Bearer {secret}")
                );
                if let Some((name, value)) = extra {
                    assert_eq!(resolved.headers()[name], value);
                }
            }
        })
    });
    for task in tasks {
        task.await.unwrap();
    }
}

#[tokio::test]
async fn reviewed_third_party_models_use_max_tokens_without_driver_forks() {
    let cases = [
        (
            OpenRouterProfile::from_api_key(CANARY)
                .unwrap()
                .build()
                .unwrap(),
            "openrouter",
            "nvidia/nemotron-3-ultra-550b-a55b:free",
        ),
        (
            DeepSeekProfile::from_api_key(CANARY)
                .unwrap()
                .build()
                .unwrap(),
            "deepseek",
            "deepseek-v4-flash",
        ),
        (
            ZaiStandardProfile::from_api_key(CANARY)
                .unwrap()
                .build()
                .unwrap(),
            "zai",
            "glm-4.7-flash",
        ),
        (
            ZaiCodingProfile::from_api_key(CANARY)
                .unwrap()
                .build()
                .unwrap(),
            "zai",
            "glm-4.7-flash",
        ),
    ];
    for (runtime, provider, model) in cases {
        let transport = MockTransport::scripted([MockExchange::response(success())]);
        let request = GenerateRequest::new(
            ModelRef::new(provider, model).unwrap(),
            vec![Message::user("fixed offline prompt")],
        )
        .with_options(GenerationOptions::new().with_max_output_tokens(16));
        LlmClient::new(runtime, transport.clone())
            .complete(request)
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(transport.captured_requests()[0].body()).unwrap();
        assert_eq!(body["max_tokens"], 16);
        assert!(body.get("max_completion_tokens").is_none());
    }
}

#[tokio::test]
async fn openrouter_profile_normalizes_one_identical_terminal_replay() {
    let runtime = OpenRouterProfile::from_api_key(CANARY)
        .unwrap()
        .build()
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let response = MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(include_bytes!(
            "fixtures/provider/compat/openrouter/text.sse"
        )))],
    );
    let transport = MockTransport::scripted([MockExchange::response(response)]);
    let request = GenerateRequest::new(
        ModelRef::new("openrouter", "nvidia/nemotron-3-ultra-550b-a55b:free").unwrap(),
        vec![Message::user("fixed offline prompt")],
    );
    let message = LlmClient::new(runtime, transport)
        .complete(request)
        .await
        .unwrap();
    assert_eq!(message.finish_reason(), &philo::FinishReason::Stop);
    assert!(message.usage().is_some());
}

fn success() -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let delta = json!({
        "id": "fixture",
        "model": "fixture-model",
        "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": null}]
    });
    let finish = json!({
        "id": "fixture",
        "model": "fixture-model",
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
