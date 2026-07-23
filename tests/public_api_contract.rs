//! Downstream-facing public API compile and source-boundary checks.

use philo::{
    AssistantStream, LlmClient, ProviderRuntime, RequestControl, ResourceLimits,
    ResourceLimitsBuilder,
};

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
        "src/provider/profiles/test_only.rs",
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
                !normalized.contains("openai_chat"),
                "private wire leaked in {relative}"
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
    // Phase-two freezes tools/tool_choice/parallel_tool_calls/reasoning/response_format.
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
        TestOnlyProfile,
    };

    let schema_value = serde_json::json!({"type": "string"});
    let root_schema = philo::ToolSchema::new(schema_value.clone()).unwrap();
    let deep_schema = DeepToolSchema::new(schema_value).unwrap();
    let _: philo::SchemaLimits = DeepSchemaLimits::official();
    assert_eq!(root_schema, deep_schema);

    let capabilities = DeepHistoryCapabilities::official_openai_defaults();
    let dialect = DeepDialectPolicy::official_openai();
    let policy = DeepHistoryPolicy::official_openai();
    let normalized = deep_normalize_history(&[], &capabilities, &dialect, &policy).unwrap();
    assert!(normalized.messages().is_empty());

    let root_official = philo::OfficialOpenAiProfile::from_api_key("root-path-key").unwrap();
    let deep_official = DeepOfficialOpenAiProfile::from_api_key("deep-path-key").unwrap();
    let _: philo::ProviderProfile = root_official.profile().unwrap();
    let _: DeepProviderProfile = deep_official.profile().unwrap();
    assert!(
        TestOnlyProfile::localhost(
            "http://127.0.0.1:8787/v1/chat/completions",
            "test-only-path-key",
        )
        .is_ok()
    );
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

#[test]
fn versioned_provider_config_has_root_and_deep_public_paths() {
    use philo::provider::config::{
        ConfigSchemaVersion as DeepVersion, ConfigSource as DeepSource, ConfigValue as DeepValue,
        ProviderConfigLayer as DeepLayer, ProviderConfigSnapshot as DeepSnapshot,
        SecretReference as DeepSecretReference,
    };

    let _: philo::ConfigSchemaVersion = DeepVersion::CURRENT;
    let source: philo::ConfigSource = DeepSource::programmatic("public-api/config").unwrap();
    let reference: philo::SecretReference =
        DeepSecretReference::environment_variable("PHILO_PUBLIC_API_KEY").unwrap();
    let layer: philo::ProviderConfigLayer =
        DeepLayer::new(source).with_credential(DeepValue::set(reference));
    let _: philo::ProviderConfigSnapshot = DeepSnapshot::official_openai()
        .unwrap()
        .merge_layers([layer])
        .unwrap();

    let error = philo::ProviderConfigError::new(
        "field",
        philo::ProviderConfigFailure::InvalidValue,
        "safe public configuration error",
    );
    assert_eq!(error.field(), "field");
}
