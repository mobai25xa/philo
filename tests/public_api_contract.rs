//! Downstream-facing public API compile and source-boundary checks.

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
        "src/domain/tools.rs",
        "src/error.rs",
        "src/observability/trace.rs",
        "src/provider/profile.rs",
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
            assert!(
                !normalized.contains("serde_json::value"),
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
        "response_format",
    ] {
        assert!(
            !generation_options.contains(forbidden),
            "non-scope request control: {forbidden}"
        );
    }
    // Phase-two freezes tools/tool_choice/parallel_tool_calls/reasoning on GenerationOptions.
    assert!(generation_options.contains("tools:"));
    assert!(generation_options.contains("tool_choice:"));
    assert!(generation_options.contains("parallel_tool_calls:"));
    assert!(generation_options.contains("reasoning:"));
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
        for forbidden in [
            "TestOnlyProfile",
            "reqwest::",
            "serde_json::Value",
            "extra_body",
            "compatible_endpoint",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden example surface {forbidden}",
                path.display()
            );
        }
    }
}
