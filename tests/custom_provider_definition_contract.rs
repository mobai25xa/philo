//! Public custom-provider definition and static registry compilation contract.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::error::ProviderRegistryFailure;
use philo::provider::auth::ApiKey;
use philo::provider::capability::ProviderCapabilities;
use philo::provider::catalog::ProductId;
use philo::provider::definition::AuthScheme;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::registry::{ProviderRegistration, ProviderRegistry};
use philo::provider::secret::{SecretReference, SecretResolver};
use philo::{
    GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef, ProviderDefinition,
    ProviderDeploymentConfig, ProviderId,
};
use proptest::prelude::*;
use support::mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};

struct CountingResolver {
    calls: AtomicUsize,
}

impl CountingResolver {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SecretResolver for CountingResolver {
    fn resolve(
        &self,
        _reference: &SecretReference,
    ) -> Result<ApiKey, philo::error::ProviderConfigError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ApiKey::new("custom-provider-secret-value")
            .map_err(|_| panic!("static test API key must be valid"))
    }
}

fn openai_definition(provider_id: ProviderId) -> philo::ProviderDefinition {
    ProviderDefinition::openai_chat(provider_id, ProductId::new("chat").unwrap())
        .with_endpoint(EndpointConfig::absolute("https://llm.example.com/v1").unwrap())
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::bearer())
        .with_capabilities(ProviderCapabilities::conservative_chat_completions())
        .allow_unregistered_models()
        .build()
        .unwrap()
}

fn anthropic_definition(provider_id: ProviderId) -> philo::ProviderDefinition {
    ProviderDefinition::anthropic_messages(provider_id, ProductId::new("messages").unwrap())
        .with_endpoint(EndpointConfig::absolute("https://llm.example.com/v1").unwrap())
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::api_key_header(HeaderName::from_static("x-api-key")).unwrap())
        .with_anthropic_version("2023-06-01")
        .unwrap()
        .with_capabilities(ProviderCapabilities::conservative_messages())
        .allow_unregistered_models()
        .build()
        .unwrap()
}

fn success(body: &'static [u8]) -> MockResponse {
    MockResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        )]),
        vec![MockBodyItem::chunk(Bytes::from_static(body))],
    )
}

#[test]
fn two_products_for_one_provider_require_explicit_product_selection() {
    let provider_id = ProviderId::new("custom-gateway").unwrap();
    let openai = openai_definition(provider_id.clone());
    let anthropic = anthropic_definition(provider_id.clone());
    let openai_protocol = openai.protocol_id().clone();
    let anthropic_protocol = anthropic.protocol_id().clone();
    let registry = ProviderRegistry::new();
    let chat_registration = registry
        .register(ProviderRegistration::from_definition(openai).unwrap())
        .unwrap();
    let messages_registration = registry
        .register(ProviderRegistration::from_definition(anthropic).unwrap())
        .unwrap();

    let reference = SecretReference::environment_variable("CUSTOM_PROVIDER_KEY").unwrap();
    let deployment = ProviderDeploymentConfig::new(provider_id.clone(), reference);
    let resolver = CountingResolver::new();
    let chat = registry
        .build_product_deployment(
            &provider_id,
            &ProductId::new("chat").unwrap(),
            &deployment,
            &resolver,
        )
        .unwrap();
    let messages = registry
        .build_product_deployment(
            &provider_id,
            &ProductId::new("messages").unwrap(),
            &deployment,
            &resolver,
        )
        .unwrap();

    assert_eq!(chat_registration.provider_id(), chat.provider_id());
    assert_eq!(chat_registration.product_id(), Some(chat.product_id()));
    assert_eq!(chat_registration.protocol_id(), Some(chat.protocol_id()));
    assert_eq!(messages_registration.provider_id(), messages.provider_id());
    assert_eq!(
        messages_registration.product_id(),
        Some(messages.product_id())
    );
    assert_eq!(
        messages_registration.protocol_id(),
        Some(messages.protocol_id())
    );
    assert_eq!(chat.protocol_id(), &openai_protocol);
    assert_eq!(messages.protocol_id(), &anthropic_protocol);
    assert_eq!(resolver.calls(), 2);
    let ambiguous = registry
        .build_deployment(&provider_id, &deployment, &resolver)
        .unwrap_err();
    assert!(matches!(
        ambiguous,
        philo::LlmError::ProviderRegistry(ref error)
            if error.reason() == ProviderRegistryFailure::AmbiguousProductSelection
    ));
    let debug = format!("{ambiguous:?}");
    assert!(!debug.contains("CUSTOM_PROVIDER_KEY"));
    assert!(!debug.contains("custom-provider-secret-value"));
    assert_eq!(resolver.calls(), 2);
    assert!(
        registry
            .remove_product(&provider_id, &ProductId::new("chat").unwrap())
            .unwrap()
            .is_some()
    );
    assert_eq!(chat.product_id().as_str(), "chat");
}

#[test]
fn deployment_provenance_fails_before_secret_resolution_and_debug_is_redacted() {
    let provider_id = ProviderId::new("custom-openai").unwrap();
    let definition = openai_definition(provider_id.clone());
    let registry = ProviderRegistry::new();
    registry
        .register(ProviderRegistration::from_definition(definition.clone()).unwrap())
        .unwrap();
    let deployment = ProviderDeploymentConfig::new(
        ProviderId::new("different-provider").unwrap(),
        SecretReference::environment_variable("SECRET_REFERENCE_CANARY").unwrap(),
    );
    let resolver = CountingResolver::new();

    assert!(
        registry
            .build_deployment(&provider_id, &deployment, &resolver)
            .is_err()
    );
    assert_eq!(resolver.calls(), 0);
    let debug = format!("{definition:?} {deployment:?} {registry:?}");
    assert!(!debug.contains("SECRET_REFERENCE_CANARY"));
    assert!(!debug.contains("custom-provider-secret-value"));
}

#[test]
fn public_builder_remains_fail_closed_for_missing_security_declarations() {
    let provider_id = ProviderId::new("incomplete-provider").unwrap();
    let product_id = ProductId::new("chat").unwrap();
    assert!(
        ProviderDefinition::openai_chat(provider_id.clone(), product_id.clone())
            .with_endpoint(EndpointConfig::absolute("https://llm.example.com/v1").unwrap())
            .with_auth_scheme(AuthScheme::bearer())
            .allow_unregistered_models()
            .build()
            .is_err()
    );
    assert!(
        ProviderDefinition::openai_chat(provider_id, product_id)
            .with_endpoint(EndpointConfig::absolute("https://llm.example.com/v1").unwrap())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            .build()
            .is_err()
    );
}

#[tokio::test]
async fn custom_definitions_reuse_both_protocol_drivers_with_distinct_wire_and_auth() {
    let provider_id = ProviderId::new("custom-driver-reuse").unwrap();
    let registry = ProviderRegistry::new();
    registry
        .register(
            ProviderRegistration::from_definition(openai_definition(provider_id.clone())).unwrap(),
        )
        .unwrap();
    registry
        .register(
            ProviderRegistration::from_definition(anthropic_definition(provider_id.clone()))
                .unwrap(),
        )
        .unwrap();
    let deployment = ProviderDeploymentConfig::new(
        provider_id.clone(),
        SecretReference::environment_variable("CUSTOM_DRIVER_REUSE_KEY").unwrap(),
    );
    let resolver = CountingResolver::new();
    let openai_runtime = registry
        .build_product_deployment(
            &provider_id,
            &ProductId::new("chat").unwrap(),
            &deployment,
            &resolver,
        )
        .unwrap();
    let anthropic_runtime = registry
        .build_product_deployment(
            &provider_id,
            &ProductId::new("messages").unwrap(),
            &deployment,
            &resolver,
        )
        .unwrap();

    let openai_transport = MockTransport::scripted([MockExchange::response(success(
        b"data: {\"id\":\"custom-openai\",\"model\":\"model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    ))]);
    let anthropic_transport = MockTransport::scripted([MockExchange::response(success(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"custom-anthropic\",\"model\":\"model\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ))]);
    let request = || {
        GenerateRequest::new(
            ModelRef::new("custom-driver-reuse", "model").unwrap(),
            vec![Message::user("contract")],
        )
        .with_options(GenerationOptions::new().with_max_output_tokens(32))
    };
    let openai = LlmClient::new(openai_runtime, openai_transport.clone());
    let anthropic = LlmClient::new(anthropic_runtime, anthropic_transport.clone());
    assert_eq!(openai.complete(request()).await.unwrap().text(), "ok");
    assert_eq!(anthropic.complete(request()).await.unwrap().text(), "ok");

    let openai_request = &openai_transport.captured_requests()[0];
    let anthropic_request = &anthropic_transport.captured_requests()[0];
    assert!(openai_request.headers().contains_key(header::AUTHORIZATION));
    assert!(anthropic_request.headers().contains_key("x-api-key"));
    assert!(
        anthropic_request
            .headers()
            .contains_key("anthropic-version")
    );
    let openai_body: serde_json::Value = serde_json::from_slice(openai_request.body()).unwrap();
    let anthropic_body: serde_json::Value =
        serde_json::from_slice(anthropic_request.body()).unwrap();
    assert_eq!(openai_body["stream"], true);
    assert_eq!(anthropic_body["max_tokens"], 32);
    assert_eq!(resolver.calls(), 2);
}

#[test]
fn bearer_and_api_key_headers_are_owned_and_cannot_be_overridden() {
    let provider_id = ProviderId::new("custom-auth-matrix").unwrap();
    let definition =
        ProviderDefinition::openai_chat(provider_id.clone(), ProductId::new("chat").unwrap())
            .with_endpoint(EndpointConfig::absolute("https://auth.example.com/v1").unwrap())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(
                AuthScheme::api_key_header(HeaderName::from_static("x-api-key")).unwrap(),
            )
            .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            .allow_unregistered_models()
            .build()
            .unwrap();
    let registry = ProviderRegistry::new();
    registry
        .register(ProviderRegistration::from_definition(definition).unwrap())
        .unwrap();
    let deployment = ProviderDeploymentConfig::new(
        provider_id.clone(),
        SecretReference::environment_variable("CUSTOM_AUTH_MATRIX_KEY").unwrap(),
    );
    let runtime = registry
        .build_deployment(&provider_id, &deployment, &CountingResolver::new())
        .unwrap();
    let resolved = runtime
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    assert!(resolved.headers().contains_key("x-api-key"));
    assert!(!resolved.headers().contains_key(header::AUTHORIZATION));

    let request = HeaderMap::from_iter([(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_static("collision-canary"),
    )]);
    let error = runtime.resolve_headers(Vec::new(), &request).unwrap_err();
    assert!(!format!("{error:?}").contains("collision-canary"));
}

#[test]
fn anthropic_custom_definition_supports_bound_bearer_auth() {
    let provider_id = ProviderId::new("custom-anthropic-bearer").unwrap();
    let definition = ProviderDefinition::anthropic_messages(
        provider_id.clone(),
        ProductId::new("messages").unwrap(),
    )
    .with_endpoint(EndpointConfig::absolute("https://bearer.example.com/v1/messages").unwrap())
    .bind_credential_to_endpoint_origin()
    .with_auth_scheme(AuthScheme::bearer())
    .with_anthropic_version("2023-06-01")
    .unwrap()
    .with_capabilities(ProviderCapabilities::conservative_messages())
    .allow_unregistered_models()
    .build()
    .unwrap();
    let registry = ProviderRegistry::new();
    registry
        .register(ProviderRegistration::from_definition(definition).unwrap())
        .unwrap();
    let deployment = ProviderDeploymentConfig::new(
        provider_id.clone(),
        SecretReference::environment_variable("CUSTOM_ANTHROPIC_BEARER_KEY").unwrap(),
    );
    let runtime = registry
        .build_deployment(&provider_id, &deployment, &CountingResolver::new())
        .unwrap();
    let resolved = runtime
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    assert!(resolved.headers().contains_key(header::AUTHORIZATION));
    assert!(resolved.headers().contains_key("anthropic-version"));
    assert!(!resolved.headers().contains_key("x-api-key"));
}

#[test]
fn production_custom_definitions_reject_non_https_and_private_origins() {
    for endpoint in [
        "http://public.example.com/v1",
        "https://127.0.0.1/v1",
        "https://localhost/v1",
        "https://[::1]/v1",
    ] {
        let result = ProviderDefinition::openai_chat(
            ProviderId::new("unsafe-endpoint").unwrap(),
            ProductId::new("chat").unwrap(),
        )
        .with_endpoint(EndpointConfig::absolute(endpoint).unwrap())
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::bearer())
        .with_capabilities(ProviderCapabilities::conservative_chat_completions())
        .allow_unregistered_models()
        .build();
        assert!(result.is_err(), "unsafe endpoint was accepted: {endpoint}");
    }
}

proptest! {
    #[test]
    fn arbitrary_endpoint_and_auth_header_input_never_panics(input in ".{0,256}") {
        let _ = EndpointConfig::absolute(&input);
        if let Ok(name) = input.parse::<HeaderName>() {
            let _ = AuthScheme::api_key_header(name);
        }
    }
}
