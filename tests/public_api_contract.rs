//! Downstream-facing phase-one public API compile and source-boundary checks.

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
        "src/domain/request.rs",
        "src/domain/event.rs",
        "src/error.rs",
        "src/observability/trace.rs",
        "src/provider/profile.rs",
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
    for forbidden in [
        "extra_body",
        "extra_json",
        "tools:",
        "reasoning:",
        "images:",
        "audio:",
        "structured_output",
        "prompt_cache",
        "retry:",
    ] {
        assert!(
            !request.contains(forbidden),
            "non-scope request control: {forbidden}"
        );
    }
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
