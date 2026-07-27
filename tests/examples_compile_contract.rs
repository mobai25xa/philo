//! P2-015 compile and quality markers for documented examples.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// `provider_config.rs` is intentionally absent: FR-005 moved layered
/// configuration into `philo-config`, and its example moved with it to
/// `crates/philo-config/examples/provider_config.rs`.
fn required_examples() -> &'static [&'static str] {
    &[
        "quickstart.rs",
        "custom_provider.rs",
        "raw_extension.rs",
        "config_separation.rs",
        "tool_single.rs",
        "tool_parallel.rs",
        "tool_reject.rs",
        "image_url.rs",
        "structured_json_schema.rs",
        "stream_text.rs",
        "complete_text.rs",
        "typed_errors.rs",
        "provider_registry.rs",
        "provider_profiles.rs",
        "provider_diagnostics.rs",
        "provider_auth_shapes.rs",
        "deployment_mapping.rs",
        "provider_routing.rs",
        "anthropic_messages.rs",
        "custom_openai_chat_provider.rs",
        "custom_anthropic_messages_provider.rs",
        "multi_provider_same_protocol.rs",
        "one_provider_multiple_protocols.rs",
    ]
}

#[test]
fn fix_root_examples_cover_the_migration_paths() {
    let root = examples_dir();
    let quickstart = fs::read_to_string(root.join("quickstart.rs")).unwrap();
    assert!(quickstart.contains("use philo::{"));
    for forbidden in ["philo::domain::", "philo::provider::", "philo::transport::"] {
        assert!(
            !quickstart.contains(forbidden),
            "quickstart bypasses the root allowlist: {forbidden}"
        );
    }

    let custom = fs::read_to_string(root.join("custom_provider.rs")).unwrap();
    assert!(custom.contains("ProviderDefinition::openai_chat"));
    assert!(custom.contains("https://llm.example.com"));

    let raw = fs::read_to_string(root.join("raw_extension.rs")).unwrap();
    assert!(raw.contains("OpenAiChatRawExtension::dangerous_from_object"));
    assert!(raw.contains("\"provider\""));

    let config = fs::read_to_string(root.join("config_separation.rs")).unwrap();
    assert!(config.contains("official_openai_inputs"));
    assert!(config.contains("definition.compile"));
}

#[test]
fn phase2_examples_exist_without_hardcoded_secrets() {
    let root = examples_dir();
    for name in required_examples() {
        let path = root.join(name);
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("missing example {}: {error}", path.display());
        });
        assert!(
            !source.contains("sk-"),
            "{name} must not embed API key material"
        );
        assert!(
            !source.contains("OPENAI_API_KEY="),
            "{name} must not hardcode credential assignment"
        );
        assert!(
            !source.contains("ANTHROPIC_API_KEY="),
            "{name} must not hardcode Anthropic credential assignment"
        );
    }
}

#[test]
fn phase2_tool_examples_keep_execution_in_application() {
    let root = examples_dir();
    let single = fs::read_to_string(root.join("tool_single.rs")).unwrap();
    assert!(single.contains("validate_tool_call"));
    assert!(single.contains("execute_weather"));
    assert!(single.contains("ToolResultMessage"));
    assert!(single.contains("normalize_history") || single.contains("from_tool_result"));

    let parallel = fs::read_to_string(root.join("tool_parallel.rs")).unwrap();
    assert!(parallel.contains("validate_tool_call"));
    assert!(parallel.contains("BTreeMap"));

    let reject = fs::read_to_string(root.join("tool_reject.rs")).unwrap();
    assert!(reject.contains("PermissionDenied"));
    assert!(reject.contains("ToolValidationFailure"));
    assert!(!reject.contains("LlmError::PermissionDenied"));
}

#[test]
fn phase2_examples_compile_with_cargo_check() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let status = Command::new("cargo")
        .args([
            "check",
            "--manifest-path",
            manifest.to_str().expect("utf-8 manifest path"),
            "--examples",
            "--all-features",
        ])
        .status()
        .expect("spawn cargo check --examples");
    assert!(
        status.success(),
        "cargo check --examples --all-features failed with {status}"
    );
}
