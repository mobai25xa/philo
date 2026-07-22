//! Executable R2-A06 ownership and dependency checks.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn source(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).unwrap()
}

fn production_source(path: &str) -> String {
    source(path)
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn rust_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

fn production_sources_under(directory: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_sources(&crate_root().join(directory), &mut files);
    files
        .into_iter()
        .map(|file| {
            let relative = file
                .strip_prefix(crate_root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let production = fs::read_to_string(file)
                .unwrap()
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .to_owned();
            (relative, production)
        })
        .collect()
}

#[test]
fn planner_is_the_only_production_history_normalization_owner() {
    let mut files = Vec::new();
    rust_sources(&crate_root().join("src"), &mut files);
    let mut owners = Vec::new();
    let mut calls = 0usize;
    for file in files {
        let relative = file.strip_prefix(crate_root()).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        let production = text.split("#[cfg(test)]").next().unwrap_or_default();
        let count = production
            .lines()
            .filter(|line| {
                line.contains("normalize_history(") && !line.contains("fn normalize_history(")
            })
            .count();
        if count > 0 {
            owners.push(relative.to_string_lossy().replace('\\', "/"));
            calls += count;
        }
    }
    assert_eq!(calls, 1);
    assert_eq!(owners, ["src/execution/planner.rs"]);
}

#[test]
fn lifecycle_uses_only_the_new_pipeline_and_old_facades_are_absent() {
    let lifecycle = production_source("src/client/lifecycle.rs");
    for required in [
        "CallPlanner::plan",
        "ProtocolDispatch::for_kind",
        "AttemptExecutor::new",
        "ResponseSession::open",
    ] {
        assert!(
            lifecycle.contains(required),
            "missing lifecycle stage: {required}"
        );
    }
    for forbidden in [
        ".ends_with(",
        "request.validate(",
        "OpenAiChatRequestAdapter",
        "decode_openai_chat_stream(",
        "ResourceLimits::official()",
    ] {
        assert!(
            !lifecycle.contains(forbidden),
            "legacy lifecycle token: {forbidden}"
        );
    }

    let mut files = Vec::new();
    rust_sources(&crate_root().join("src"), &mut files);
    let all = files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "OpenAiChatRequestAdapter",
        "EncodedOpenAiChatRequest",
        "decode_openai_chat_stream_with_limits",
        "pub(crate) fn decode_openai_chat_stream(",
    ] {
        assert!(
            !all.contains(forbidden),
            "legacy symbol remains: {forbidden}"
        );
    }
}

#[test]
fn execution_and_protocol_layers_keep_their_dependency_boundaries() {
    for file in [
        "src/execution/planner.rs",
        "src/execution/executor.rs",
        "src/protocol/openai_chat/driver.rs",
        "src/protocol/openai_chat/state.rs",
    ] {
        assert!(
            !production_source(file).contains("ResourceLimits::official()"),
            "production default limit lookup in {file}"
        );
    }

    let driver = production_source("src/protocol/openai_chat/driver.rs");
    for forbidden in [
        "crate::transport",
        "AuthProvider",
        "ProviderRuntime",
        "OfficialOpenAiProfile",
        "Transport",
    ] {
        assert!(
            !driver.contains(forbidden),
            "driver dependency leak: {forbidden}"
        );
    }

    let executor = production_source("src/execution/executor.rs");
    for forbidden in [
        "ChatCompletionChunkWire",
        "ToolCallDeltaWire",
        "MessageRole",
        "ResponseFormat",
        "validate_structured_response",
        "openai_chat",
    ] {
        assert!(
            !executor.contains(forbidden),
            "executor semantic leak: {forbidden}"
        );
    }

    let mut transport_files = Vec::new();
    rust_sources(&crate_root().join("src/transport"), &mut transport_files);
    for file in transport_files {
        let text = fs::read_to_string(&file).unwrap();
        for forbidden in [
            "crate::protocol",
            "ToolCall",
            "MessageRole",
            "ResponseFormat",
        ] {
            assert!(
                !text.contains(forbidden),
                "transport dependency leak in {}: {forbidden}",
                file.display()
            );
        }
    }
}

#[test]
fn domain_is_provider_protocol_and_transport_independent() {
    for (file, text) in production_sources_under("src/domain") {
        for forbidden in ["crate::provider", "crate::protocol", "crate::transport"] {
            assert!(
                !text.contains(forbidden),
                "domain dependency leak in {file}: {forbidden}"
            );
        }
    }
}

#[test]
fn production_default_limit_lookups_have_an_explicit_file_allowlist() {
    let owners = production_sources_under("src")
        .into_iter()
        .filter_map(|(file, text)| text.contains("ResourceLimits::official()").then_some(file))
        .collect::<BTreeSet<_>>();
    let allowed = BTreeSet::from([
        "src/domain/content.rs".to_owned(),
        "src/domain/request.rs".to_owned(),
        "src/domain/schema.rs".to_owned(),
        "src/domain/tools.rs".to_owned(),
        "src/provider/profile.rs".to_owned(),
    ]);
    assert_eq!(owners, allowed);
}

#[test]
fn protocol_and_client_do_not_branch_on_provider_brand() {
    for directory in ["src/protocol", "src/client"] {
        for (file, text) in production_sources_under(directory) {
            for forbidden in [
                "official-openai",
                "provider().as_str()",
                "provider_id.as_str()",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "provider-brand branch token in {file}: {forbidden}"
                );
            }
        }
    }
}

#[test]
fn public_request_api_has_no_untyped_wire_extension_escape_hatch() {
    for (file, text) in production_sources_under("src") {
        assert!(
            !text.lines().any(|line| {
                line.contains("pub ")
                    && (line.contains("extra_body") || line.contains("extra_headers"))
            }),
            "public untyped extension field in {file}"
        );
        if file != "src/protocol/openai_chat/wire.rs" {
            assert!(
                !text.contains("serde(flatten)"),
                "serde flatten outside private response wire allowlist: {file}"
            );
        }
    }
}
