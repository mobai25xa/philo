//! Provider Registry, Factory, and immutable Runtime contracts.

use std::sync::{Arc, Barrier};
use std::thread;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    AnthropicMessagesOptions, AnthropicRawExtension, ApiKey, ConfigSource, ConfigValue,
    EndpointSpec, GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef,
    OfficialOpenAiFactory, ProviderConfigFailure, ProviderConfigLayer, ProviderConfigSnapshot,
    ProviderId, ProviderRegistration, ProviderRegistry, ProviderRegistryFailure,
    ProviderRequestOptions, ProviderRuntimeFactory, SecretReference, SecretResolver,
};

const KEY_CANARY: &str = "philo-registry-secret-canary-1734";
const SECRET_NAME: &str = "PHILO_REGISTRY_API_KEY";

#[derive(Clone)]
struct StaticResolver {
    key: ApiKey,
}

impl StaticResolver {
    fn new() -> Self {
        Self::with_key(KEY_CANARY)
    }

    fn with_key(key: &str) -> Self {
        Self {
            key: ApiKey::new(key).unwrap(),
        }
    }
}

impl SecretResolver for StaticResolver {
    fn resolve(&self, reference: &SecretReference) -> Result<ApiKey, philo::ProviderConfigError> {
        if reference.name() == SECRET_NAME {
            Ok(self.key.clone())
        } else {
            Err(philo::ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::SecretUnavailable,
                "test resolver has no matching secret",
            ))
        }
    }
}

fn official_snapshot() -> ProviderConfigSnapshot {
    let layer = ProviderConfigLayer::new(
        ConfigSource::environment_secret("env/registry", SECRET_NAME).unwrap(),
    )
    .with_credential(ConfigValue::set(
        SecretReference::environment_variable(SECRET_NAME).unwrap(),
    ));
    ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([layer])
        .unwrap()
}

fn anthropic_snapshot() -> ProviderConfigSnapshot {
    let layer = ProviderConfigLayer::new(
        ConfigSource::environment_secret("env/registry", SECRET_NAME).unwrap(),
    )
    .with_credential(ConfigValue::set(
        SecretReference::environment_variable(SECRET_NAME).unwrap(),
    ));
    ProviderConfigSnapshot::official_anthropic()
        .unwrap()
        .merge_layers([layer])
        .unwrap()
}

#[test]
fn official_anthropic_factory_is_explicit_and_diagnostics_are_value_free() {
    let registry = ProviderRegistry::with_official_profiles().unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
    let provider_id = ProviderId::new("official-anthropic").unwrap();
    let runtime = registry
        .build(&provider_id, &anthropic_snapshot(), &StaticResolver::new())
        .unwrap();
    assert_eq!(runtime.protocol_id().as_str(), "anthropic-messages");
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.anthropic.com/v1/messages"
    );

    let raw = AnthropicRawExtension::dangerous_from_object(
        serde_json::json!({"future_feature": "diagnostic-secret-canary"}),
    )
    .unwrap();
    let request = GenerateRequest::new(
        ModelRef::new("official-anthropic", "claude-sonnet-5").unwrap(),
        vec![Message::user("message-secret-canary")],
    )
    .with_options(
        GenerationOptions::new()
            .with_protocol_options(AnthropicMessagesOptions::new().with_raw_extension(raw)),
    );
    let diagnostics = runtime
        .diagnostics_for_request(&request, &ProviderRequestOptions::new(), "2026-07-26")
        .unwrap();
    assert!(diagnostics.compat().is_empty());
    assert_eq!(
        diagnostics.typed_extensions(),
        ["anthropic-messages-options", "non_portable_extension_used"]
    );
    let debug = format!("{diagnostics:?}");
    assert!(!debug.contains("diagnostic-secret-canary"));
    assert!(!debug.contains("message-secret-canary"));
}

fn registration(version: &str) -> ProviderRegistration {
    ProviderRegistration::new("official-openai", version, OfficialOpenAiFactory).unwrap()
}

fn success_response(generation_id: &str) -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from(format!(
            concat!(
                "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"ok\"}},\"finish_reason\":null}}]}}\n\n",
                "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n",
                "data: [DONE]\n\n"
            ),
            generation_id, generation_id
        )))],
    )
}

fn anthropic_success_response() -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(include_bytes!(
            "fixtures/phase-5/anthropic-messages/stream/text.sse"
        )))],
    )
}

#[tokio::test]
async fn official_anthropic_runtime_dispatches_end_to_end_without_protocol_guessing() {
    let registry = ProviderRegistry::with_official_anthropic().unwrap();
    let provider_id = ProviderId::new("official-anthropic").unwrap();
    let runtime = registry
        .build(&provider_id, &anthropic_snapshot(), &StaticResolver::new())
        .unwrap();
    let transport = MockTransport::scripted([MockExchange::response(anthropic_success_response())]);
    let client = LlmClient::new(runtime, transport.clone());
    client
        .complete(GenerateRequest::new(
            ModelRef::new("official-anthropic", "claude-sonnet-5").unwrap(),
            vec![Message::user("Hello")],
        ))
        .await
        .unwrap();
    transport.assert_consumed();
    let captured = transport.captured_requests();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].headers()["x-api-key"], KEY_CANARY);
    assert_eq!(captured[0].headers()["anthropic-version"], "2023-06-01");
    assert!(!captured[0].headers().contains_key("anthropic-beta"));
    let body: serde_json::Value = serde_json::from_slice(captured[0].body()).unwrap();
    assert_eq!(body["model"], "claude-sonnet-5");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["stream"], true);
}

#[test]
fn duplicate_registration_is_rejected_unless_replace_is_explicit() {
    let registry = ProviderRegistry::new();
    registry.register(registration("1.0")).unwrap();
    let error = registry.register(registration("1.1")).unwrap_err();
    assert_eq!(
        error.reason(),
        ProviderRegistryFailure::DuplicateRegistration
    );
    assert_eq!(error.provider_id(), Some("official-openai"));

    let previous = registry.replace(registration("1.1")).unwrap();
    assert_eq!(previous.version(), "1.0");
    assert_eq!(
        registry
            .get_by_name(" OFFICIAL-OPENAI ")
            .unwrap()
            .unwrap()
            .version(),
        "1.1"
    );
}

#[test]
fn registry_listing_is_deterministic_and_value_free() {
    let registry = ProviderRegistry::new();
    registry
        .register(ProviderRegistration::new("zeta", "1.0", OfficialOpenAiFactory).unwrap())
        .unwrap();
    registry
        .register(ProviderRegistration::new("Alpha", "2.0", OfficialOpenAiFactory).unwrap())
        .unwrap();
    let entries = registry.list().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.provider_id().as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    let debug = format!("{registry:?}");
    assert!(debug.contains("alpha"));
    assert!(!debug.contains(KEY_CANARY));
}

#[test]
fn runtime_survives_registry_removal_as_an_immutable_snapshot() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = ProviderId::new("official-openai").unwrap();
    let runtime = registry
        .build(&provider_id, &official_snapshot(), &StaticResolver::new())
        .unwrap();
    assert_eq!(runtime.provider_id(), &provider_id);
    registry.remove(&provider_id).unwrap();
    assert!(registry.get(&provider_id).unwrap().is_none());
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.openai.com/v1/chat/completions"
    );
}

#[test]
fn replacement_does_not_mutate_existing_runtime() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = ProviderId::new("official-openai").unwrap();
    let snapshot = official_snapshot();
    let first = registry
        .build(&provider_id, &snapshot, &StaticResolver::new())
        .unwrap();
    assert_eq!(
        registry.replace(registration("2.0")).unwrap().version(),
        "1.0"
    );
    let second = registry
        .build(&provider_id, &snapshot, &StaticResolver::new())
        .unwrap();
    assert_eq!(first.endpoint(), second.endpoint());
    assert_eq!(first.provider_id(), second.provider_id());
}

#[tokio::test]
async fn shared_transport_does_not_share_auth_headers_or_request_state() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = ProviderId::new("official-openai").unwrap();
    let first_key = "philo-registry-first-key";
    let second_key = "philo-registry-second-key";
    let first_runtime = registry
        .build(
            &provider_id,
            &official_snapshot(),
            &StaticResolver::with_key(first_key),
        )
        .unwrap();
    let second_runtime = registry
        .build(
            &provider_id,
            &official_snapshot(),
            &StaticResolver::with_key(second_key),
        )
        .unwrap();
    let transport = MockTransport::scripted([
        MockExchange::response(success_response("registry-first")),
        MockExchange::response(success_response("registry-second")),
    ]);
    let first_client = LlmClient::new(first_runtime, transport.clone());
    let second_client = LlmClient::new(second_runtime, transport.clone());
    let request = || {
        GenerateRequest::new(
            ModelRef::new("official-openai", "gpt-test").unwrap(),
            vec![Message::user("registry isolation")],
        )
    };
    let (first, second) = tokio::join!(
        first_client.complete(request()),
        second_client.complete(request())
    );
    first.unwrap();
    second.unwrap();
    transport.assert_consumed();
    let captured = transport.captured_requests();
    assert_eq!(captured.len(), 2);
    assert_ne!(
        captured[0].local_request_id(),
        captured[1].local_request_id()
    );
    let mut authorization = captured
        .iter()
        .map(|request| {
            request.headers()[header::AUTHORIZATION]
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    authorization.sort();
    let mut expected = vec![
        format!("Bearer {first_key}"),
        format!("Bearer {second_key}"),
    ];
    expected.sort();
    assert_eq!(authorization, expected);
}

#[test]
fn factory_reports_config_source_without_secret_value() {
    let layer =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/endpoint").unwrap())
            .with_endpoint(ConfigValue::set(EndpointSpec::base_and_path(
                "https://example.com/v1",
                "/chat/completions",
            )));
    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([layer])
        .unwrap();
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = ProviderId::new("official-openai").unwrap();
    let error = registry
        .build(&provider_id, &snapshot, &StaticResolver::new())
        .unwrap_err();
    let error = match error {
        philo::LlmError::ProviderConfig(error) => error,
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(error.source(), Some("application/endpoint"));
    assert!(!error.to_string().contains(KEY_CANARY));
}

#[test]
fn official_profile_builds_through_the_same_factory() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = ProviderId::new("official-openai").unwrap();
    let snapshot = official_snapshot();
    let runtime = registry
        .build(&provider_id, &snapshot, &StaticResolver::new())
        .unwrap();
    let legacy = philo::OfficialOpenAiProfile::new(ApiKey::new(KEY_CANARY).unwrap())
        .build()
        .unwrap();
    assert_eq!(runtime.provider_id(), legacy.provider_id());
    assert_eq!(runtime.protocol_id(), legacy.protocol_id());
    assert_eq!(runtime.endpoint(), legacy.endpoint());
    assert_eq!(runtime.dialect(), legacy.dialect());
    assert_eq!(runtime.transport_options(), legacy.transport_options());
}

struct BlockingFactory {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl ProviderRuntimeFactory for BlockingFactory {
    fn build(
        &self,
        config: &ProviderConfigSnapshot,
        resolver: &dyn SecretResolver,
    ) -> Result<philo::ProviderRuntime, philo::LlmError> {
        self.entered.wait();
        self.release.wait();
        OfficialOpenAiFactory.build(config, resolver)
    }
}

#[test]
fn registry_releases_lock_before_factory_callback() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let registry = ProviderRegistry::new();
    registry
        .register(
            ProviderRegistration::new(
                "official-openai",
                "1.0",
                BlockingFactory {
                    entered: entered.clone(),
                    release: release.clone(),
                },
            )
            .unwrap(),
        )
        .unwrap();
    let thread_registry = registry.clone();
    let build = thread::spawn(move || {
        let provider_id = ProviderId::new("official-openai").unwrap();
        thread_registry
            .build(&provider_id, &official_snapshot(), &StaticResolver::new())
            .unwrap()
    });
    entered.wait();
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(
        registry.replace(registration("2.0")).unwrap().version(),
        "1.0"
    );
    release.wait();
    build.join().unwrap();
}

#[test]
fn registry_rejects_mismatched_config_before_factory_and_preserves_source() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let layer =
        ProviderConfigLayer::new(ConfigSource::programmatic("application/provider").unwrap())
            .with_provider_id(ConfigValue::set("other-provider".to_owned()));
    let snapshot = ProviderConfigSnapshot::official_openai()
        .unwrap()
        .merge_layers([layer])
        .unwrap();
    let error = registry
        .build(
            &ProviderId::new("official-openai").unwrap(),
            &snapshot,
            &StaticResolver::new(),
        )
        .unwrap_err();
    let error = match error {
        philo::LlmError::ProviderConfig(error) => error,
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(error.source(), Some("application/provider"));
    assert_eq!(error.reason(), ProviderConfigFailure::InvalidValue);
    assert!(
        registry
            .get(&ProviderId::new("official-openai").unwrap())
            .unwrap()
            .is_some()
    );
}
