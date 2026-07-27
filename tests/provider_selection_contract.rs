//! Provider identity comes from explicit declaration only (FR-006).
//!
//! `provider/detection.rs` used to guess a provider from an endpoint URL and sat
//! below every explicit source as an enabled-by-default fallback. Guessing wrong
//! aims a request at the wrong product; guessing right saves one declaration. The
//! second path is gone, so this file pins the remaining one: the precedence chain
//! over declared sources, and an unambiguous failure when nothing is declared.

use philo::ProviderId;
use philo::provider::definition::AuthScheme;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::factory::{ProviderSelectionInput, ProviderSelectionSource, ProviderSelector};
use philo::provider::{ProductId, ProviderCapabilities, ProviderDefinition, TestOnlyProfile};
use philo::transport::mock::MockTransport;
use philo::{GenerateRequest, LlmClient, Message, ModelRef};

fn provider(value: &str) -> ProviderId {
    ProviderId::new(value).unwrap()
}

#[test]
fn precedence_runs_request_then_model_then_config_then_profile() {
    let request = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_request_provider(provider("request-provider"))
            .with_model_provider(provider("model-provider"))
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider")),
    );
    assert_eq!(request.provider_id().unwrap().as_str(), "request-provider");
    assert_eq!(request.source(), ProviderSelectionSource::RequestExplicit);

    let model = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_model_provider(provider("model-provider"))
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider")),
    );
    assert_eq!(model.provider_id().unwrap().as_str(), "model-provider");
    assert_eq!(model.source(), ProviderSelectionSource::ModelExplicit);

    let configured = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider")),
    );
    assert_eq!(
        configured.provider_id().unwrap().as_str(),
        "configured-provider"
    );
    assert_eq!(
        configured.source(),
        ProviderSelectionSource::ProviderExplicit
    );

    let profile = ProviderSelector::select(
        &ProviderSelectionInput::new().with_built_in_profile(provider("built-in-provider")),
    );
    assert_eq!(profile.provider_id().unwrap().as_str(), "built-in-provider");
    assert_eq!(profile.source(), ProviderSelectionSource::BuiltInProfile);
}

#[test]
fn nothing_declared_selects_nothing_and_never_falls_back() {
    let selection = ProviderSelector::select(&ProviderSelectionInput::new());
    assert!(selection.provider_id().is_none());
    assert_eq!(selection.source(), ProviderSelectionSource::Undeclared);
}

/// An endpoint URL that used to be detected as `official-openai` now yields nothing.
///
/// The reviewed rule was an exact match on `api.openai.com` + `/v1/chat/completions`.
/// The selector no longer looks at endpoints at all, so the URL is not an input.
#[test]
fn an_official_looking_endpoint_no_longer_implies_a_provider() {
    let selection = ProviderSelector::select(&ProviderSelectionInput::new());
    assert!(selection.provider_id().is_none());
    assert_ne!(
        selection.provider_id().map(ProviderId::as_str),
        Some("official-openai")
    );
}

/// The runtime is the layer that turns "undeclared" into a hard failure.
#[tokio::test]
async fn a_request_for_an_undeclared_provider_fails_before_transport() {
    let runtime = TestOnlyProfile::localhost("http://127.0.0.1:41994/v1/chat/completions", "key")
        .unwrap()
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let foreign = GenerateRequest::new(
        ModelRef::new("undeclared-provider", "some-model").unwrap(),
        vec![Message::user("hello")],
    );
    let error = LlmClient::new(runtime, mock.clone())
        .stream(foreign)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        philo::LlmError::Validation(ref error)
            if error.reason() == philo::error::ValidationReason::ProviderMismatch
    ));
    assert!(mock.captured_requests().is_empty());
}

/// Declaring the provider is the whole migration: one `ProviderDefinition` call.
///
/// The retired detector held exactly one reviewed rule —
/// `api.openai.com` + `/v1/chat/completions` → `official-openai` /
/// `chat-completions`. Expressing it explicitly is this, and it is the only
/// place the identity now comes from.
#[test]
fn declaring_the_provider_is_the_supported_replacement() {
    let definition = ProviderDefinition::openai_chat(
        provider("official-openai"),
        ProductId::new("chat-completions").unwrap(),
    )
    .with_endpoint(
        EndpointConfig::base_and_path("https://api.openai.com/v1", "/chat/completions").unwrap(),
    )
    .bind_credential_to_endpoint_origin()
    .with_auth_scheme(AuthScheme::bearer())
    .with_capabilities(ProviderCapabilities::conservative_chat_completions())
    .allow_unregistered_models()
    .build()
    .unwrap();
    assert_eq!(definition.provider_id().as_str(), "official-openai");
    assert_eq!(definition.product_id().as_str(), "chat-completions");
}
