//! Endpoint detection precedence, normalization, explanation, and no-I/O contracts.

use philo::{
    DetectionConfidence, DetectionUnknownReason, EndpointDetectionPolicy, NormalizedEndpointFacts,
    ProviderConfigField, ProviderConfigSnapshot, ProviderId, ProviderSelectionInput,
    ProviderSelectionSource, ProviderSelector,
};

fn provider(value: &str) -> ProviderId {
    ProviderId::new(value).unwrap()
}

fn official_facts() -> NormalizedEndpointFacts {
    NormalizedEndpointFacts::parse("https://api.openai.com/v1/chat/completions").unwrap()
}

#[test]
fn explicit_provider_model_or_profile_always_wins_over_detection() {
    let facts = official_facts();
    let request = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_request_provider(provider("request-provider"))
            .with_model_provider(provider("model-provider"))
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider"))
            .with_endpoint(facts.clone()),
    );
    assert_eq!(request.provider_id().unwrap().as_str(), "request-provider");
    assert_eq!(request.source(), ProviderSelectionSource::RequestExplicit);
    assert!(request.detection().is_none());

    let model = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_model_provider(provider("model-provider"))
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider"))
            .with_endpoint(facts.clone()),
    );
    assert_eq!(model.source(), ProviderSelectionSource::ModelExplicit);

    let configured = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_provider(provider("configured-provider"))
            .with_built_in_profile(provider("built-in-provider"))
            .with_endpoint(facts.clone()),
    );
    assert_eq!(
        configured.source(),
        ProviderSelectionSource::ProviderExplicit
    );

    let snapshot = ProviderConfigSnapshot::official_openai().unwrap();
    let provenance = snapshot
        .provenance(ProviderConfigField::ProviderId)
        .unwrap();
    let traced = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_provider_from_config(provider("official-openai"), provenance)
            .with_endpoint(facts.clone()),
    );
    assert_eq!(
        traced.config_source().unwrap().as_str(),
        provenance.source().id().as_str()
    );

    let profile = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_built_in_profile(provider("built-in-provider"))
            .with_endpoint(facts),
    );
    assert_eq!(profile.source(), ProviderSelectionSource::BuiltInProfile);
}

#[test]
fn custom_domains_ip_literals_and_reverse_proxies_remain_unknown() {
    for endpoint in [
        "https://openai.internal.example/v1/chat/completions",
        "https://127.0.0.1/v1/chat/completions",
        "https://[::1]/v1/chat/completions",
        "https://evil-openai.com/v1/chat/completions",
    ] {
        let selection = ProviderSelector::select(
            &ProviderSelectionInput::new()
                .with_endpoint(NormalizedEndpointFacts::parse(endpoint).unwrap()),
        );
        assert!(selection.provider_id().is_none(), "{endpoint}");
        assert_eq!(selection.source(), ProviderSelectionSource::ProtocolDefault);
    }
}

#[test]
fn hostname_case_trailing_dot_port_and_idn_are_normalized_safely() {
    let normalized = NormalizedEndpointFacts::parse(
        "https://API.OPENAI.COM.:443/v1/chat/completions/?ignored=secret#fragment",
    )
    .unwrap();
    assert_eq!(normalized.host(), "api.openai.com");
    assert_eq!(normalized.path(), "/v1/chat/completions");
    assert_eq!(normalized.port(), None);
    let selection =
        ProviderSelector::select(&ProviderSelectionInput::new().with_endpoint(normalized));
    assert_eq!(selection.provider_id().unwrap().as_str(), "official-openai");
    assert_eq!(
        selection.source(),
        ProviderSelectionSource::EndpointDetection
    );
    assert_eq!(
        selection.detection().unwrap().confidence(),
        Some(DetectionConfidence::Exact)
    );

    let idn =
        NormalizedEndpointFacts::parse("https://api.openaï.example/v1/chat/completions").unwrap();
    let unknown = ProviderSelector::select(&ProviderSelectionInput::new().with_endpoint(idn));
    assert_eq!(
        unknown.detection().unwrap().unknown_reason(),
        Some(DetectionUnknownReason::InternationalizedHost)
    );
}

#[test]
fn suffix_matching_requires_dns_label_boundaries() {
    for endpoint in [
        "https://api.openai.com.evil.example/v1/chat/completions",
        "https://evil-openai.com/v1/chat/completions",
        "https://notapi.openai.com/v1/chat/completions",
    ] {
        let selection = ProviderSelector::select(
            &ProviderSelectionInput::new()
                .with_endpoint(NormalizedEndpointFacts::parse(endpoint).unwrap()),
        );
        assert!(selection.provider_id().is_none(), "{endpoint}");
    }
}

#[test]
fn detection_performs_no_dns_http_or_secret_access_and_explanation_is_safe() {
    const CANARY: &str = "endpoint-secret-canary";
    let facts = NormalizedEndpointFacts::parse(&format!(
        "https://api.openai.com/v1/chat/completions?api_key={CANARY}#{CANARY}"
    ))
    .unwrap();
    let selection = ProviderSelector::select(&ProviderSelectionInput::new().with_endpoint(facts));
    let explanation = selection.detection().unwrap();
    assert_eq!(
        explanation.rule_id(),
        Some("builtin.official-openai.chat-completions.v1")
    );
    assert_eq!(explanation.host(), Some("api.openai.com"));
    assert_eq!(explanation.product_path(), Some("/v1/chat/completions"));
    assert!(!format!("{selection:?}").contains(CANARY));
}

#[test]
fn detection_can_be_disabled_and_unknown_remains_protocol_default() {
    let disabled = ProviderSelector::select(
        &ProviderSelectionInput::new()
            .with_endpoint(official_facts())
            .with_detection_policy(EndpointDetectionPolicy::Disabled),
    );
    assert!(disabled.provider_id().is_none());
    assert_eq!(disabled.source(), ProviderSelectionSource::ProtocolDefault);
    assert_eq!(
        disabled.detection().unwrap().unknown_reason(),
        Some(DetectionUnknownReason::Disabled)
    );

    let missing = ProviderSelector::select(&ProviderSelectionInput::new());
    assert!(missing.provider_id().is_none());
    assert_eq!(
        missing.detection().unwrap().unknown_reason(),
        Some(DetectionUnknownReason::MissingEndpoint)
    );
}

#[test]
fn port_path_and_query_cannot_expand_detection_authority() {
    for endpoint in [
        "https://api.openai.com:8443/v1/chat/completions",
        "https://api.openai.com/v1/responses",
        "http://api.openai.com/v1/chat/completions",
    ] {
        let selection = ProviderSelector::select(
            &ProviderSelectionInput::new()
                .with_endpoint(NormalizedEndpointFacts::parse(endpoint).unwrap()),
        );
        assert!(selection.provider_id().is_none(), "{endpoint}");
    }
}
