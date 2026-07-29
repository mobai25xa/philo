//! Downstream-facing public API compile and source-boundary checks.

use philo::domain::limits::{ResourceLimits, ResourceLimitsBuilder};
use philo::{AssistantStream, LlmClient, ProviderRuntime, RequestControl};

fn assert_send_sync<T: Send + Sync>() {}
fn assert_send_unpin<T: Send + Unpin>() {}

#[test]
fn client_runtime_and_controls_keep_the_native_async_contract() {
    assert_send_sync::<LlmClient>();
    assert_send_sync::<ProviderRuntime>();
    assert_send_sync::<RequestControl>();
    assert_send_unpin::<AssistantStream>();
}

#[test]
fn resource_limits_builder_is_the_downstream_construction_path() {
    let builder: ResourceLimitsBuilder = ResourceLimits::builder()
        .with_max_messages(128)
        .with_max_structured_output_bytes(2 * 1024 * 1024);
    let limits = builder.build().unwrap();
    assert_eq!(limits.max_messages, 128);
    assert_eq!(limits.max_structured_output_bytes, 2 * 1024 * 1024);
    assert_eq!(
        limits.max_request_body_bytes,
        ResourceLimits::official().max_request_body_bytes
    );
}

#[test]
fn extensible_auth_and_header_policy_have_root_and_deep_public_paths() {
    use philo::provider::auth::{
        ApiKeyHeaderAuth as DeepApiKeyHeaderAuth, DynamicAuth as DeepDynamicAuth,
        DynamicCredentialSource as DeepDynamicCredentialSource,
    };
    use philo::provider::headers::{
        ClientIdentityFragment as DeepClientIdentityFragment,
        DynamicHeaderPolicy as DeepDynamicHeaderPolicy,
        DynamicHeaderSource as DeepDynamicHeaderSource,
    };

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<philo::provider::auth::DynamicAuth>();
    assert_send_sync::<DeepDynamicAuth>();
    assert_send_sync::<philo::provider::headers::DynamicHeaderPolicy>();
    assert_send_sync::<DeepDynamicHeaderPolicy>();
    let _: Option<philo::provider::auth::ApiKeyHeaderAuth> = None::<DeepApiKeyHeaderAuth>;
    let _: Option<philo::provider::headers::ClientIdentityFragment> =
        None::<DeepClientIdentityFragment>;
    let _: Option<&dyn philo::provider::auth::DynamicCredentialSource> =
        None::<&dyn DeepDynamicCredentialSource>;
    let _: Option<&dyn philo::provider::headers::DynamicHeaderSource> =
        None::<&dyn DeepDynamicHeaderSource>;
}

#[test]
fn primary_public_surface_does_not_name_private_implementation_types() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let public_sources = [
        "src/lib.rs",
        "src/client/lifecycle.rs",
        "src/domain/mod.rs",
        "src/domain/content.rs",
        "src/domain/ids.rs",
        "src/domain/message.rs",
        "src/domain/request.rs",
        "src/domain/event.rs",
        "src/domain/schema/mod.rs",
        "src/domain/schema/budget.rs",
        "src/domain/history/mod.rs",
        "src/domain/history/diagnostics.rs",
        "src/domain/history/normalize.rs",
        "src/domain/history/policy.rs",
        "src/domain/history/replay.rs",
        "src/domain/tools.rs",
        "src/error.rs",
        "src/observability/trace.rs",
        "src/provider/profile.rs",
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/official_anthropic.rs",
        "src/provider/profiles/test_only.rs",
        "src/protected.rs",
        "src/protocol_options/mod.rs",
        "src/protocol_options/anthropic.rs",
        "src/protocol_options/openai.rs",
        "src/provider/capability.rs",
        "src/provider/runtime.rs",
        "src/transport/mod.rs",
    ];
    for relative in public_sources {
        let source = std::fs::read_to_string(root.join(relative)).unwrap();
        for line in source
            .lines()
            .filter(|line| line.trim_start().starts_with("pub "))
        {
            let normalized = line.to_ascii_lowercase();
            for implementation_type in [
                "reqwest::client",
                "reqwest::request",
                "reqwest::response",
                "reqwest::error",
            ] {
                assert!(
                    !normalized.contains(implementation_type),
                    "reqwest type leaked in {relative}"
                );
            }
            // Structured output intentionally exposes serde_json::Value on the frozen
            // AssistantMessage / collector surface. All other public lines stay free of it.
            let structured_output_surface = normalized.contains("structured_output")
                || normalized.contains("collect_assistant_message_for_format");
            assert!(
                structured_output_surface || !normalized.contains("serde_json::value"),
                "JSON value leaked in {relative}"
            );
            assert!(
                !normalized.contains("openai_chat::"),
                "private wire leaked in {relative}"
            );
            assert!(
                !normalized.contains("anthropic_messages::wire"),
                "private Anthropic wire leaked in {relative}"
            );
            assert!(
                !normalized.contains("wire::"),
                "private wire leaked in {relative}"
            );
        }
    }
}

#[test]
fn request_api_has_no_arbitrary_body_or_non_scope_controls() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let request = std::fs::read_to_string(root.join("src/domain/request.rs")).unwrap();
    let options_start = request.find("pub struct GenerationOptions").unwrap();
    let options_end = request[options_start..]
        .find("pub struct GenerateRequest")
        .map(|offset| options_start + offset)
        .unwrap();
    let generation_options = &request[options_start..options_end];
    for forbidden in [
        "extra_body",
        "extra_json",
        "images:",
        "audio:",
        "prompt_cache",
        "retry:",
    ] {
        assert!(
            !generation_options.contains(forbidden),
            "non-scope request control: {forbidden}"
        );
    }
    // The stable capability contract includes tools, reasoning, and response formats.
    assert!(generation_options.contains("tools:"));
    assert!(generation_options.contains("tool_choice:"));
    assert!(generation_options.contains("parallel_tool_calls:"));
    assert!(generation_options.contains("reasoning:"));
    assert!(generation_options.contains("response_format:"));
}

#[test]
fn production_examples_use_only_official_profile_and_public_types() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in [
            "TestOnlyProfile",
            "reqwest::",
            "serde_json::Value",
            "extra_body",
            "compatible_endpoint",
        ] {
            assert!(
                !production.contains(forbidden),
                "{} contains forbidden example surface {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn schema_history_and_provider_root_and_deep_paths_remain_compatible() {
    use philo::domain::history::{
        DialectPolicy as DeepDialectPolicy, HistoryCapabilities as DeepHistoryCapabilities,
        HistoryPolicy as DeepHistoryPolicy, normalize_history as deep_normalize_history,
    };
    use philo::domain::schema::{SchemaLimits as DeepSchemaLimits, ToolSchema as DeepToolSchema};
    use philo::provider::{
        OfficialOpenAiProfile as DeepOfficialOpenAiProfile, ProviderProfile as DeepProviderProfile,
    };
    use philo_presets::{
        DeepSeekProfile as DeepDeepSeekProfile, OpenRouterProfile as DeepOpenRouterProfile,
        ZaiCodingProfile as DeepZaiCodingProfile, ZaiStandardProfile as DeepZaiStandardProfile,
    };

    let schema_value = serde_json::json!({"type": "string"});
    let root_schema = philo::domain::schema::ToolSchema::new(schema_value.clone()).unwrap();
    let deep_schema = DeepToolSchema::new(schema_value).unwrap();
    let _: philo::domain::schema::SchemaLimits = DeepSchemaLimits::official();
    assert_eq!(root_schema, deep_schema);

    let capabilities = DeepHistoryCapabilities::official_openai_defaults();
    let dialect = DeepDialectPolicy::official_openai();
    let policy = DeepHistoryPolicy::official_openai();
    let normalized = deep_normalize_history(&[], &capabilities, &dialect, &policy).unwrap();
    assert!(normalized.messages().is_empty());

    let root_official =
        philo::provider::profiles::OfficialOpenAiProfile::from_api_key("root-path-key").unwrap();
    let deep_official = DeepOfficialOpenAiProfile::from_api_key("deep-path-key").unwrap();
    let _: philo::provider::profile::ProviderProfile = root_official.profile().unwrap();
    let _: DeepProviderProfile = deep_official.profile().unwrap();
    let _: DeepProviderProfile = DeepOpenRouterProfile::from_api_key("deep-openrouter")
        .unwrap()
        .profile()
        .unwrap();
    let _: DeepProviderProfile = DeepDeepSeekProfile::from_api_key("deep-deepseek")
        .unwrap()
        .profile()
        .unwrap();
    let _: DeepProviderProfile = DeepZaiStandardProfile::from_api_key("deep-zai-standard")
        .unwrap()
        .profile()
        .unwrap();
    let _: DeepProviderProfile = DeepZaiCodingProfile::from_api_key("deep-zai-coding")
        .unwrap()
        .profile()
        .unwrap();
}

#[test]
fn migration_helpers_remain_absent_from_public_facades() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in ["src/lib.rs", "src/domain/mod.rs", "src/provider/mod.rs"] {
        let source = std::fs::read_to_string(root.join(relative)).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap_or_default();
        for private_name in [
            "ProviderProfileParts",
            "ChatStateMachine",
            "OpenAiChatStreamContext",
            "ProtocolDispatch",
            "PreparedCall",
            "CompiledSchemaMetadata",
        ] {
            assert!(
                !production.contains(private_name),
                "private migration helper leaked through {relative}: {private_name}"
            );
        }
    }
}

/// FR-005 moved the versioned-configuration surface out of the core. What the
/// core still owes is the secret boundary — a reference type it can hold
/// without ever holding a value — and the error type both sides report through.
#[test]
fn the_secret_boundary_has_root_and_deep_public_paths() {
    use philo::provider::secret::{
        EnvironmentSecretResolver as DeepEnvResolver, SecretReference as DeepSecretReference,
        SecretResolver as DeepResolver,
    };

    let reference: philo::provider::secret::SecretReference =
        DeepSecretReference::environment_variable("PHILO_PUBLIC_API_KEY").unwrap();
    let resolver: &dyn DeepResolver = &DeepEnvResolver;
    assert!(resolver.resolve(&reference).is_err());
    assert!(!format!("{reference:?}").contains("value"));

    let error = philo::error::ProviderConfigError::new(
        "field",
        philo::error::ProviderConfigFailure::InvalidValue,
        "safe public configuration error",
    );
    assert_eq!(error.field(), "field");
}

#[test]
fn provider_registry_definition_public_paths_are_additive() {
    use philo::provider::{
        OfficialOpenAiProfile as DeepOfficialOpenAi, ProviderRegistration as DeepRegistration,
        ProviderRegistry as DeepRegistry,
    };

    // A registration is a definition, and nothing else. The configuration
    // factory trait it used to accept lives outside the core now (FR-005).
    let registration: philo::provider::registry::ProviderRegistration =
        DeepRegistration::from_definition(DeepOfficialOpenAi::definition().unwrap()).unwrap();
    let registry: philo::provider::registry::ProviderRegistry = DeepRegistry::new();
    registry.register(registration).unwrap();
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(
        DeepRegistry::with_official_profiles()
            .unwrap()
            .list()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn anthropic_public_api_is_typed_additive_and_wire_private() {
    use philo::protocol_options::{
        AnthropicEffort as DeepEffort, AnthropicMessagesOptions as DeepOptions,
        AnthropicThinkingDisplay as DeepDisplay, ProtocolOptions as DeepProtocolOptions,
    };
    use philo::provider::OfficialAnthropicProfile as DeepProfile;

    let options: philo::protocol_options::AnthropicMessagesOptions = DeepOptions::new()
        .with_adaptive_thinking(DeepDisplay::Omitted)
        .with_effort(DeepEffort::High);
    let protocol_options: philo::ProtocolOptions = options.clone().into();
    let _: DeepProtocolOptions = protocol_options.clone();
    assert_eq!(
        protocol_options.protocol_id(),
        philo::protocol_options::ANTHROPIC_MESSAGES_PROTOCOL_ID
    );
    let request = philo::GenerateRequest::new(
        philo::ModelRef::new("official-anthropic", "claude-sonnet-5").unwrap(),
        vec![philo::Message::user("public compile contract")],
    )
    .with_options(
        philo::GenerationOptions::new()
            .with_max_output_tokens(64)
            .with_protocol_options(options),
    );
    assert!(request.options().protocol_options().is_some());

    let root =
        philo::provider::profiles::OfficialAnthropicProfile::from_api_key("root-path-placeholder")
            .unwrap();
    let deep = DeepProfile::from_api_key("deep-path-placeholder").unwrap();
    let _: philo::provider::profile::ProviderProfile = root.profile().unwrap();
    let _: philo::provider::profile::ProviderProfile = deep.profile().unwrap();

    let facade = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
    )
    .unwrap();
    for wire in [
        "MessagesRequestWire",
        "MessageStartEventWire",
        "ContentBlockDeltaWire",
    ] {
        assert!(!facade.contains(wire), "Anthropic wire leaked: {wire}");
    }
}

#[test]
fn model_catalog_and_typed_compat_have_root_and_deep_public_paths() {
    use philo::provider::catalog::{
        CatalogSource as DeepCatalogSource, CatalogSourceId as DeepCatalogSourceId,
        ModelCatalog as DeepModelCatalog, ProductId as DeepProductId,
    };
    use philo::provider::protocol_contract::{
        AnthropicUsageCompat as DeepAnthropicUsageCompat, CompatProfile as DeepCompatProfile,
        MaxOutputTokensWireFormat as DeepTokenFormat,
    };

    let _: philo::provider::catalog::ProductId = DeepProductId::new("chat-completions").unwrap();
    let _: philo::provider::catalog::CatalogSource = DeepCatalogSource::new(
        DeepCatalogSourceId::new("contract").unwrap(),
        "2026-07-23",
        None::<String>,
    )
    .unwrap();
    let _: philo::provider::catalog::ModelCatalog = DeepModelCatalog::default();
    // The core exposes only the resolved contract and performs no sparse
    // declaration layering.
    let resolved: philo::provider::protocol_contract::CompatProfile =
        DeepCompatProfile::openai_chat_default().with_max_output_tokens(
            DeepTokenFormat::MaxTokens,
            philo::domain::history::PolicySource::ProviderProfile,
        );
    assert_eq!(
        resolved.request().max_output_tokens,
        philo::provider::protocol_contract::MaxOutputTokensWireFormat::MaxTokens
    );
    assert_eq!(
        resolved.source(philo::provider::protocol_contract::CompatField::RequestMaxOutputTokens),
        philo::domain::history::PolicySource::ProviderProfile
    );
    let _: philo::provider::protocol_contract::AnthropicUsageCompat =
        DeepAnthropicUsageCompat::AllowMonotonicStableFields;
}

#[test]
fn endpoint_mapping_and_model_body_policy_have_root_and_deep_public_paths() {
    use philo::provider::endpoint::{
        EndpointConfig as DeepConfig, EndpointQuery as DeepQuery,
        EndpointQuerySource as DeepQuerySource, EndpointTemplate as DeepTemplate,
        QueryMergeRule as DeepMergeRule,
    };

    let query: philo::provider::endpoint::EndpointQuery = DeepQuery::new()
        .with_set(
            "api-version",
            "2026-07-01",
            DeepMergeRule::Override,
            DeepQuerySource::ProductProfile,
        )
        .unwrap();
    let template: philo::provider::endpoint::EndpointTemplate =
        DeepTemplate::parse("deployments/{deployment}/chat/completions").unwrap();
    let _: philo::provider::endpoint::EndpointConfig =
        DeepConfig::base_and_template("https://api.openai.com/v1", template, query).unwrap();
    let _: philo::provider::protocol_contract::ModelBodyWireFormat =
        philo::provider::protocol_contract::ModelBodyWireFormat::Omit;
    let _: philo::provider::endpoint::EndpointNetworkPolicy =
        philo::provider::endpoint::EndpointNetworkPolicy::public_https();
}

#[test]
fn provider_selection_has_typed_deep_public_paths() {
    use philo::provider::factory::{
        ProviderSelectionInput as DeepSelectionInput,
        ProviderSelectionSource as DeepSelectionSource, ProviderSelector as DeepSelector,
    };

    let input: philo::provider::factory::ProviderSelectionInput = DeepSelectionInput::new()
        .with_provider(philo::ProviderId::new("declared-provider").unwrap());
    let selection: philo::provider::factory::ProviderSelection = DeepSelector::select(&input);
    assert_eq!(
        selection.provider_id().unwrap().as_str(),
        "declared-provider"
    );
    assert_eq!(selection.source(), DeepSelectionSource::ProviderExplicit);

    let undeclared: philo::provider::factory::ProviderSelection =
        DeepSelector::select(&DeepSelectionInput::new());
    assert!(undeclared.provider_id().is_none());
    assert_eq!(undeclared.source(), DeepSelectionSource::Undeclared);
}

#[test]
fn capability_decisions_and_catalog_evidence_have_deep_public_paths() {
    use philo::domain::request::CapabilityStatus as DeepCapabilityStatus;
    use philo::provider::catalog::CatalogSource as DeepCatalogSource;

    let runtime = philo_presets::OpenRouterProfile::from_api_key("public-api-placeholder")
        .unwrap()
        .build()
        .unwrap();
    let model = philo::ModelId::new("nvidia/nemotron-3-ultra-550b-a55b:free").unwrap();
    let entry = runtime.model_entry(&model).unwrap();

    let decision: DeepCapabilityStatus = entry.support_status;
    let evidence: &DeepCatalogSource = &entry.source;
    assert_eq!(decision, DeepCapabilityStatus::Supported);
    assert_eq!(
        evidence.id().as_str(),
        "openrouter-official-docs-reviewed-2026-07-23"
    );
    assert!(!evidence.is_stale_on("2026-07-24").unwrap());
    assert_eq!(
        entry
            .field_source("capabilities.function_tools")
            .unwrap()
            .id(),
        evidence.id()
    );
}
