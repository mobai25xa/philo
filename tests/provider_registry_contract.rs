//! Provider Registry and immutable Runtime contracts.
//!
//! FR-005 removed the configuration-document compiler from the core, so a
//! registration is now exactly one thing: a secret-free [`ProviderDefinition`].
//! A runtime is reached the single way — definition plus deployment plus an
//! explicit secret resolution. These tests pin that path and the isolation
//! guarantees that survived the extraction.

use std::sync::{Arc, Barrier};
use std::thread;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::request::CapabilityStatus;
use philo::error::{ProviderConfigFailure, ProviderRegistryFailure};
use philo::provider::auth::ApiKey;
use philo::provider::profiles::{OfficialAnthropicProfile, OfficialOpenAiProfile};
use philo::provider::registry::{ProviderRegistration, ProviderRegistry};
use philo::provider::secret::{SecretReference, SecretResolver};
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    GenerateRequest, LlmClient, Message, ModelId, ModelRef, ProviderDeploymentConfig, ProviderId,
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
    fn resolve(
        &self,
        reference: &SecretReference,
    ) -> Result<ApiKey, philo::error::ProviderConfigError> {
        if reference.name() == SECRET_NAME {
            Ok(self.key.clone())
        } else {
            Err(philo::error::ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::SecretUnavailable,
                "test resolver has no matching secret",
            ))
        }
    }
}

fn provider(value: &str) -> ProviderId {
    ProviderId::new(value).unwrap()
}

fn deployment(value: &str) -> ProviderDeploymentConfig {
    ProviderDeploymentConfig::new(
        provider(value),
        SecretReference::environment_variable(SECRET_NAME).unwrap(),
    )
}

fn openai_registration() -> ProviderRegistration {
    ProviderRegistration::from_definition(OfficialOpenAiProfile::definition().unwrap()).unwrap()
}

#[test]
fn official_anthropic_definition_is_explicit_and_evidence_is_separate() {
    let registry = ProviderRegistry::with_official_profiles().unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
    let provider_id = provider("official-anthropic");
    let runtime = registry
        .build_deployment(
            &provider_id,
            &deployment("official-anthropic"),
            &StaticResolver::new(),
        )
        .unwrap();
    assert_eq!(runtime.protocol_id().as_str(), "anthropic-messages");
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.anthropic.com/v1/messages"
    );

    let model = ModelId::new("claude-sonnet-5").unwrap();
    let entry = runtime.model_entry(&model).unwrap();
    assert_eq!(
        entry.support_status,
        CapabilityStatus::Supported,
        "availability is a three-state decision"
    );
    assert_eq!(entry.source.id().as_str(), "anthropic-models-ledger");
    assert!(!entry.source.is_stale_on("2026-07-26").unwrap());
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
    let runtime = registry
        .build_deployment(
            &provider("official-anthropic"),
            &deployment("official-anthropic"),
            &StaticResolver::new(),
        )
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
    registry.register(openai_registration()).unwrap();
    let error = registry.register(openai_registration()).unwrap_err();
    assert_eq!(
        error.reason(),
        ProviderRegistryFailure::DuplicateRegistration
    );
    assert_eq!(error.provider_id(), Some("official-openai"));

    let previous = registry.replace(openai_registration()).unwrap();
    assert_eq!(previous.provider_id().as_str(), "official-openai");
    assert_eq!(
        registry
            .get_by_name(" OFFICIAL-OPENAI ")
            .unwrap()
            .unwrap()
            .product_id()
            .unwrap()
            .as_str(),
        "chat-completions"
    );
}

#[test]
fn replacing_an_unregistered_provider_is_rejected() {
    let registry = ProviderRegistry::new();
    let error = registry.replace(openai_registration()).unwrap_err();
    assert_eq!(
        error.reason(),
        ProviderRegistryFailure::RegistrationNotFound
    );
}

#[test]
fn registry_listing_is_deterministic_and_value_free() {
    let registry = ProviderRegistry::with_official_profiles().unwrap();
    let entries = registry.list().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.provider_id().as_str())
            .collect::<Vec<_>>(),
        vec!["official-anthropic", "official-openai"]
    );
    let debug = format!("{registry:?}");
    assert!(debug.contains("official-openai"));
    assert!(!debug.contains(KEY_CANARY));
}

#[test]
fn runtime_survives_registry_removal_as_an_immutable_snapshot() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = provider("official-openai");
    let runtime = registry
        .build_deployment(
            &provider_id,
            &deployment("official-openai"),
            &StaticResolver::new(),
        )
        .unwrap();
    assert_eq!(runtime.provider_id(), &provider_id);
    assert!(registry.remove(&provider_id).unwrap().is_some());
    assert!(registry.get(&provider_id).unwrap().is_none());
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.openai.com/v1/chat/completions"
    );
}

#[test]
fn replacement_does_not_mutate_existing_runtime() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = provider("official-openai");
    let first = registry
        .build_deployment(
            &provider_id,
            &deployment("official-openai"),
            &StaticResolver::new(),
        )
        .unwrap();
    registry.replace(openai_registration()).unwrap();
    let second = registry
        .build_deployment(
            &provider_id,
            &deployment("official-openai"),
            &StaticResolver::new(),
        )
        .unwrap();
    assert_eq!(first.endpoint(), second.endpoint());
    assert_eq!(first.provider_id(), second.provider_id());
}

#[tokio::test]
async fn shared_transport_does_not_share_auth_headers_or_request_state() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let provider_id = provider("official-openai");
    let first_key = "philo-registry-first-key";
    let second_key = "philo-registry-second-key";
    let first_runtime = registry
        .build_deployment(
            &provider_id,
            &deployment("official-openai"),
            &StaticResolver::with_key(first_key),
        )
        .unwrap();
    let second_runtime = registry
        .build_deployment(
            &provider_id,
            &deployment("official-openai"),
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
fn a_deployment_for_a_different_provider_is_rejected_before_secret_resolution() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let error = registry
        .build_deployment(
            &provider("official-openai"),
            &deployment("some-other-provider"),
            &StaticResolver::new(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        philo::LlmError::ProviderRegistry(ref error)
            if error.reason() == ProviderRegistryFailure::FactoryProviderMismatch
    ));
}

#[test]
fn an_unresolvable_secret_fails_and_never_reaches_a_runtime() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let wrong_name = ProviderDeploymentConfig::new(
        provider("official-openai"),
        SecretReference::environment_variable("PHILO_REGISTRY_WRONG_NAME").unwrap(),
    );
    let error = registry
        .build_deployment(
            &provider("official-openai"),
            &wrong_name,
            &StaticResolver::new(),
        )
        .unwrap_err();
    let philo::LlmError::ProviderConfig(error) = error else {
        panic!("unexpected error kind")
    };
    assert_eq!(error.reason(), ProviderConfigFailure::SecretUnavailable);
    assert!(!error.to_string().contains(KEY_CANARY));
}

#[test]
fn registry_built_runtime_matches_the_preset_built_runtime() {
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let runtime = registry
        .build_deployment(
            &provider("official-openai"),
            &deployment("official-openai"),
            &StaticResolver::new(),
        )
        .unwrap();
    let preset = OfficialOpenAiProfile::new(ApiKey::new(KEY_CANARY).unwrap())
        .build()
        .unwrap();
    assert_eq!(runtime.provider_id(), preset.provider_id());
    assert_eq!(runtime.product_id(), preset.product_id());
    assert_eq!(runtime.protocol_id(), preset.protocol_id());
    assert_eq!(runtime.endpoint(), preset.endpoint());
    assert_eq!(runtime.dialect(), preset.dialect());
    assert_eq!(runtime.transport_options(), preset.transport_options());
}

#[test]
fn official_anthropic_registry_and_preset_agree_on_the_frozen_identity() {
    let registry = ProviderRegistry::with_official_anthropic().unwrap();
    let runtime = registry
        .build_deployment(
            &provider("official-anthropic"),
            &deployment("official-anthropic"),
            &StaticResolver::new(),
        )
        .unwrap();
    let preset = OfficialAnthropicProfile::new(ApiKey::new(KEY_CANARY).unwrap())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(runtime.provider_id(), preset.provider_id());
    assert_eq!(runtime.product_id(), preset.product_id());
    assert_eq!(runtime.endpoint(), preset.endpoint());
    assert_eq!(runtime.dialect(), preset.dialect());
    assert_eq!(
        runtime.catalog().entries().count(),
        preset.catalog().entries().count()
    );
}

/// The registry must not hold its lock across credential resolution.
///
/// The old proof used a blocking `ProviderRuntimeFactory`; that trait was the
/// configuration path and is gone. Secret resolution is now the user-supplied
/// callback on the compile path, so blocking there proves the same property.
struct BlockingResolver {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
    inner: StaticResolver,
}

impl SecretResolver for BlockingResolver {
    fn resolve(
        &self,
        reference: &SecretReference,
    ) -> Result<ApiKey, philo::error::ProviderConfigError> {
        self.entered.wait();
        self.release.wait();
        self.inner.resolve(reference)
    }
}

#[test]
fn registry_releases_its_lock_before_resolving_a_secret() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let registry = ProviderRegistry::with_official_openai().unwrap();
    let thread_registry = registry.clone();
    let resolver = BlockingResolver {
        entered: entered.clone(),
        release: release.clone(),
        inner: StaticResolver::new(),
    };
    let build = thread::spawn(move || {
        thread_registry
            .build_deployment(
                &provider("official-openai"),
                &deployment("official-openai"),
                &resolver,
            )
            .unwrap()
    });
    entered.wait();
    // The registry is fully usable while the resolver is blocked.
    assert_eq!(registry.list().unwrap().len(), 1);
    registry.replace(openai_registration()).unwrap();
    release.wait();
    let runtime = build.join().unwrap();
    assert_eq!(runtime.provider_id().as_str(), "official-openai");
}
