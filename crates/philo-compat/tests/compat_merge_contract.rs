//! Layered compatibility declaration and merge (FR-004).
//!
//! The core carries one already-resolved contract per model. Turning ordered
//! sparse declarations into that contract is this crate's whole job, so the
//! determinism and provenance guarantees that used to live in the core's
//! `provider_compat_contract` suite live here now.

use philo::domain::PolicySource;
use philo::provider::protocol_contract::{
    CompatField, MaxOutputTokensWireFormat, ModelBodyWireFormat, ToolArgumentsCompat, UsageCompat,
};
use philo_compat::{CompatPatch, resolve_compat};

#[test]
fn merge_is_fieldwise_deterministic_and_traced() {
    let provider = CompatPatch::from_source(PolicySource::ProviderProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens);
    let model = CompatPatch::from_source(PolicySource::ModelProfile)
        .with_tool_arguments(ToolArgumentsCompat::StringOrObject);
    let resolved = resolve_compat(&[provider, model]);

    assert_eq!(
        resolved.request().max_output_tokens,
        MaxOutputTokensWireFormat::MaxTokens
    );
    assert_eq!(
        resolved.response().tool_arguments,
        ToolArgumentsCompat::StringOrObject
    );
    assert_eq!(
        resolved.source(CompatField::RequestMaxOutputTokens),
        PolicySource::ProviderProfile
    );
    assert_eq!(
        resolved.source(CompatField::ResponseToolArguments),
        PolicySource::ModelProfile
    );
    // A leaf no layer touched still reports the protocol default.
    assert_eq!(
        resolved.source(CompatField::RequestImage),
        PolicySource::ProtocolDefault
    );
}

#[test]
fn later_layers_win_per_leaf_and_carry_their_own_source() {
    let base = CompatPatch::from_source(PolicySource::ProviderProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens)
        .with_model_body(ModelBodyWireFormat::Omit);
    let later = CompatPatch::from_source(PolicySource::ModelProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxCompletionTokens);
    let resolved = resolve_compat(&[base, later]);

    // The later layer overrode one leaf and left the other alone.
    assert_eq!(
        resolved.request().max_output_tokens,
        MaxOutputTokensWireFormat::MaxCompletionTokens
    );
    assert_eq!(resolved.request().model_body, ModelBodyWireFormat::Omit);
    assert_eq!(
        resolved.source(CompatField::RequestMaxOutputTokens),
        PolicySource::ModelProfile
    );
    assert_eq!(
        resolved.source(CompatField::RequestModelBody),
        PolicySource::ProviderProfile
    );
}

#[test]
fn the_same_layers_always_resolve_to_the_same_contract() {
    let layers = || {
        vec![
            CompatPatch::from_source(PolicySource::ProviderProfile)
                .with_usage(UsageCompat::OpenAiDropInconsistentReasoning),
            CompatPatch::from_source(PolicySource::ModelProfile)
                .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens),
        ]
    };
    assert_eq!(resolve_compat(&layers()), resolve_compat(&layers()));
}

#[test]
fn an_empty_layer_set_resolves_to_the_protocol_default() {
    let resolved = resolve_compat(&[]);
    assert_eq!(
        resolved.request().max_output_tokens,
        MaxOutputTokensWireFormat::MaxCompletionTokens
    );
    assert_eq!(resolved.response().usage, UsageCompat::OpenAi);
    for field in CompatField::all() {
        assert_eq!(resolved.source(field), PolicySource::ProtocolDefault);
    }
    assert!(CompatPatch::from_source(PolicySource::ProviderProfile).is_empty());
}

/// The point of the extraction: what this crate produces is exactly what the
/// core's definition builder accepts, with no core-side merging in between.
#[test]
fn a_resolved_contract_is_accepted_by_the_core_definition_builder() {
    use philo::provider::catalog::ProductId;
    use philo::provider::definition::AuthScheme;
    use philo::provider::endpoint::EndpointConfig;
    use philo::provider::{ProviderCapabilities, ProviderDefinition};

    let resolved = resolve_compat(&[CompatPatch::from_source(PolicySource::ProviderProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens)]);
    let definition = ProviderDefinition::openai_chat(
        philo::ProviderId::new("gateway").unwrap(),
        ProductId::new("chat").unwrap(),
    )
    .with_endpoint(
        EndpointConfig::absolute("https://gateway.example.com/v1/chat/completions").unwrap(),
    )
    .bind_credential_to_endpoint_origin()
    .with_auth_scheme(AuthScheme::bearer())
    .with_capabilities(ProviderCapabilities::conservative_chat_completions())
    .allow_unregistered_models()
    .with_openai_chat_compat(resolved)
    .build()
    .unwrap();
    assert_eq!(definition.provider_id().as_str(), "gateway");
}
