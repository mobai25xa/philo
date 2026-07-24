//! Executable architecture ownership and dependency checks.

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
    files.sort();
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

fn assert_production_file(path: &str) {
    let full = crate_root().join(path);
    assert!(full.is_file(), "missing production owner: {path}");
    let production = production_source(path);
    let meaningful_lines = production
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with("//") && !matches!(*line, "{" | "}" | ");")
        })
        .count();
    assert!(
        meaningful_lines >= 2,
        "production owner is empty or comment-only: {path}"
    );
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
        "src/protocol/openai_chat/response/machine.rs",
        "src/protocol/openai_chat/response/stream.rs",
        "src/protocol/openai_chat/response/terminal.rs",
        "src/protocol/openai_chat/response/tool_calls.rs",
        "src/protocol/openai_chat/response/usage.rs",
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
fn openai_response_uses_one_private_module_tree() {
    let root = crate_root();
    let required = [
        "src/protocol/openai_chat/response/mod.rs",
        "src/protocol/openai_chat/response/stream.rs",
        "src/protocol/openai_chat/response/machine.rs",
        "src/protocol/openai_chat/response/tool_calls.rs",
        "src/protocol/openai_chat/response/usage.rs",
        "src/protocol/openai_chat/response/terminal.rs",
    ];
    for path in required {
        assert!(root.join(path).is_file(), "missing response module: {path}");
    }
    assert!(
        !root.join("src/protocol/openai_chat/state.rs").exists(),
        "legacy response state.rs remains"
    );

    let module = production_source("src/protocol/openai_chat/mod.rs");
    assert!(module.contains("mod response;"));
    assert!(!module.contains("mod state;"));

    for (file, text) in production_sources_under("src/protocol/openai_chat/response") {
        for forbidden in [
            "ProviderRuntime",
            "OfficialOpenAiProfile",
            "TestOnlyProfile",
            "AuthProvider",
        ] {
            assert!(
                !text.contains(forbidden),
                "response dependency leak in {file}: {forbidden}"
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
fn history_uses_one_domain_only_module_tree() {
    let root = crate_root();
    for path in [
        "src/domain/history/mod.rs",
        "src/domain/history/policy.rs",
        "src/domain/history/diagnostics.rs",
        "src/domain/history/normalize.rs",
        "src/domain/history/replay.rs",
    ] {
        assert!(root.join(path).is_file(), "missing history module: {path}");
    }
    assert!(
        !root.join("src/domain/history.rs").exists(),
        "legacy domain/history.rs remains"
    );

    for (file, text) in production_sources_under("src/domain/history") {
        for forbidden in [
            "crate::provider",
            "crate::protocol",
            "crate::transport",
            "OfficialOpenAiProfile",
            "TestOnlyProfile",
            "provider_id.as_str()",
            "openrouter",
            "deepseek",
            "z.ai",
            "reqwest",
        ] {
            assert!(
                !text
                    .to_ascii_lowercase()
                    .contains(&forbidden.to_ascii_lowercase()),
                "history dependency or brand leak in {file}: {forbidden}"
            );
        }
    }
}

#[test]
fn provider_generic_contract_and_presets_are_physically_separate() {
    let root = crate_root();
    for path in [
        "src/provider/profile.rs",
        "src/provider/profiles/mod.rs",
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/test_only.rs",
    ] {
        assert!(
            root.join(path).is_file(),
            "missing provider profile module: {path}"
        );
    }

    let generic = production_source("src/provider/profile.rs");
    for forbidden in ["struct OfficialOpenAiProfile", "struct TestOnlyProfile"] {
        assert!(
            !generic.contains(forbidden),
            "preset remains in generic profile: {forbidden}"
        );
    }
    assert!(generic.contains("pub(super) struct ProviderProfileParts"));
    assert!(!generic.contains("pub(crate) struct ProviderProfileParts"));
    assert!(!generic.contains("pub struct ProviderProfileParts"));

    let runtime = production_source("src/provider/runtime.rs");
    for forbidden in ["profiles::", "OfficialOpenAiProfile", "TestOnlyProfile"] {
        assert!(
            !runtime.contains(forbidden),
            "runtime depends on concrete preset: {forbidden}"
        );
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
        "src/domain/schema/budget.rs".to_owned(),
        "src/domain/tools.rs".to_owned(),
        "src/provider/profiles/official_openai.rs".to_owned(),
        "src/provider/profiles/test_only.rs".to_owned(),
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

#[test]
fn catalog_and_typed_compat_have_single_owners_and_no_provider_brand_branches() {
    for path in [
        "src/provider/catalog/mod.rs",
        "src/provider/catalog/entry.rs",
        "src/provider/catalog/ids.rs",
        "src/provider/catalog/source.rs",
        "src/provider/catalog/merge.rs",
        "src/provider/catalog/validate.rs",
        "src/provider/compat/mod.rs",
        "src/provider/compat/profile.rs",
        "src/provider/compat/request.rs",
        "src/provider/compat/response.rs",
        "src/provider/compat/history.rs",
        "src/provider/compat/merge.rs",
        "src/provider/compat/validate.rs",
        "src/protocol/openai_chat/compat/mod.rs",
        "src/protocol/openai_chat/compat/request.rs",
        "src/protocol/openai_chat/compat/response.rs",
        "src/protocol/openai_chat/compat/error.rs",
    ] {
        assert_production_file(path);
    }

    for directory in ["src/provider/catalog", "src/provider/compat"] {
        for (file, text) in production_sources_under(directory) {
            for forbidden in [
                "official-openai",
                "test-only",
                "openrouter",
                "deepseek",
                "z.ai",
            ] {
                assert!(
                    !text.to_ascii_lowercase().contains(forbidden),
                    "provider brand leaked into generic policy owner {file}: {forbidden}"
                );
            }
        }
    }

    let driver = production_source("src/protocol/openai_chat/driver.rs");
    assert!(!driver.contains("MaxOutputTokensWireFormat"));
    assert!(!driver.contains("ToolArgumentsCompat"));
}

#[test]
fn completed_phase_2_5_layout_exists_and_legacy_files_are_absent() {
    for path in [
        "src/protocol/openai_chat/response/mod.rs",
        "src/protocol/openai_chat/response/stream.rs",
        "src/protocol/openai_chat/response/machine.rs",
        "src/protocol/openai_chat/response/tool_calls.rs",
        "src/protocol/openai_chat/response/usage.rs",
        "src/protocol/openai_chat/response/terminal.rs",
        "src/domain/schema/mod.rs",
        "src/domain/schema/compile.rs",
        "src/domain/schema/reference.rs",
        "src/domain/schema/validate.rs",
        "src/domain/schema/budget.rs",
        "src/domain/history/mod.rs",
        "src/domain/history/policy.rs",
        "src/domain/history/diagnostics.rs",
        "src/domain/history/normalize.rs",
        "src/domain/history/replay.rs",
        "src/provider/profiles/mod.rs",
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/test_only.rs",
    ] {
        assert_production_file(path);
    }

    for path in [
        "src/protocol/openai_chat/state.rs",
        "src/domain/schema.rs",
        "src/domain/history.rs",
    ] {
        assert!(
            !crate_root().join(path).exists(),
            "legacy production owner remains: {path}"
        );
    }
}

#[test]
fn response_submodules_keep_ownership_boundaries() {
    for (file, text) in production_sources_under("src/protocol/openai_chat/response") {
        for forbidden in [
            "crate::client",
            "crate::execution",
            "ProviderRuntime",
            "OfficialOpenAiProfile",
            "TestOnlyProfile",
            "AuthProvider",
            "reqwest",
        ] {
            assert!(
                !text.contains(forbidden),
                "response ownership leak in {file}: {forbidden}"
            );
        }
    }

    let stream = production_source("src/protocol/openai_chat/response/stream.rs");
    for forbidden in [
        "validate_structured_response",
        "ToolCallAccumulator",
        "ToolArguments::",
    ] {
        assert!(
            !stream.contains(forbidden),
            "stream owns response semantics: {forbidden}"
        );
    }

    let tool_calls = production_source("src/protocol/openai_chat/response/tool_calls.rs");
    for forbidden in ["crate::client", "crate::execution", "crate::transport"] {
        assert!(
            !tool_calls.contains(forbidden),
            "tool accumulator dependency leak: {forbidden}"
        );
    }

    let usage = production_source("src/protocol/openai_chat/response/usage.rs");
    for forbidden in [
        "crate::client",
        "crate::execution",
        "crate::provider",
        "crate::transport",
    ] {
        assert!(
            !usage.contains(forbidden),
            "usage dependency leak: {forbidden}"
        );
    }

    let terminal = production_source("src/protocol/openai_chat/response/terminal.rs");
    for forbidden in [
        "crate::transport",
        "HttpRequest",
        "ProviderRuntime",
        "reqwest",
    ] {
        assert!(
            !terminal.contains(forbidden),
            "terminal network/profile leak: {forbidden}"
        );
    }

    let module = production_source("src/protocol/openai_chat/mod.rs");
    assert_eq!(
        module
            .matches("decode_openai_chat_stream_with_plan")
            .count(),
        1,
        "openai_chat must expose exactly one response decoder route"
    );
    for legacy in [
        "mod state;",
        "decode_openai_chat_stream_with_limits",
        "pub(crate) fn decode_openai_chat_stream(",
    ] {
        assert!(
            !module.contains(legacy),
            "legacy decoder route remains: {legacy}"
        );
    }
}

#[test]
fn schema_and_history_stay_pure_domain_without_remote_resolvers() {
    let mut owners = production_sources_under("src/domain/history");
    owners.extend(production_sources_under("src/domain/schema"));
    owners.sort_by(|left, right| left.0.cmp(&right.0));

    for (file, text) in owners {
        for forbidden in [
            "crate::provider",
            "crate::protocol",
            "crate::transport",
            "reqwest",
            "tokio::fs",
            "std::fs",
            "url::Url",
            "resolve_remote",
            "OfficialOpenAiProfile",
            "TestOnlyProfile",
            "openrouter",
            "deepseek",
            "z.ai",
        ] {
            assert!(
                !text
                    .to_ascii_lowercase()
                    .contains(&forbidden.to_ascii_lowercase()),
                "domain algorithm dependency leak in {file}: {forbidden}"
            );
        }
    }
}

#[test]
fn provider_runtime_and_core_pipeline_are_preset_independent() {
    let runtime = production_source("src/provider/runtime.rs");
    for forbidden in [
        "profiles::",
        "OfficialOpenAiProfile",
        "TestOnlyProfile",
        "official-openai",
        "test-only",
    ] {
        assert!(
            !runtime.contains(forbidden),
            "generic runtime depends on preset: {forbidden}"
        );
    }

    for directory in [
        "src/client",
        "src/protocol",
        "src/execution",
        "src/domain/history",
    ] {
        for (file, text) in production_sources_under(directory) {
            for forbidden in [
                "OfficialOpenAiProfile",
                "TestOnlyProfile",
                "official-openai",
                "openrouter",
                "deepseek",
                "z.ai",
                "provider_id.as_str()",
            ] {
                assert!(
                    !text
                        .to_ascii_lowercase()
                        .contains(&forbidden.to_ascii_lowercase()),
                    "preset or provider-brand control flow in {file}: {forbidden}"
                );
            }
        }
    }
}

#[test]
fn private_migration_types_are_not_reexported() {
    for facade in ["src/lib.rs", "src/domain/mod.rs", "src/provider/mod.rs"] {
        let text = production_source(facade);
        for forbidden in [
            "ProviderProfileParts",
            "ChatStateMachine",
            "OpenAiChatStreamContext",
            "ProtocolDispatch",
            "PreparedCall",
            "ProtocolDriver",
            "CompiledSchemaMetadata",
        ] {
            assert!(
                !text.contains(forbidden),
                "private migration type re-exported by {facade}: {forbidden}"
            );
        }
    }

    let profile = production_source("src/provider/profile.rs");
    assert!(profile.contains("pub(super) struct ProviderProfileParts"));
    assert!(profile.contains("pub(super) fn from_parts"));
    for forbidden in [
        "pub struct ProviderProfileParts",
        "pub(crate) struct ProviderProfileParts",
        "pub fn from_parts",
        "pub(crate) fn from_parts",
    ] {
        assert!(
            !profile.contains(forbidden),
            "internal ProviderProfile seam visibility widened: {forbidden}"
        );
    }

    let schema_compile = production_source("src/domain/schema/compile.rs");
    assert!(schema_compile.contains("pub(super) struct CompiledSchemaMetadata"));
    for forbidden in [
        "pub struct CompiledSchemaMetadata",
        "pub(crate) struct CompiledSchemaMetadata",
    ] {
        assert!(
            !schema_compile.contains(forbidden),
            "compiled schema metadata visibility widened: {forbidden}"
        );
    }
}

#[test]
fn provider_config_has_one_network_free_versioned_module_tree() {
    for path in [
        "src/provider/config/mod.rs",
        "src/provider/config/schema.rs",
        "src/provider/config/source.rs",
        "src/provider/config/merge.rs",
        "src/provider/config/secret_ref.rs",
        "src/provider/config/validate.rs",
    ] {
        assert_production_file(path);
    }

    let module = production_source("src/provider/mod.rs");
    assert!(module.contains("pub mod config;"));
    let config = production_sources_under("src/provider/config");
    for (file, text) in &config {
        for forbidden in [
            "crate::client",
            "crate::execution",
            "crate::protocol",
            "crate::transport",
            "reqwest",
            "std::env::vars(",
            "std::env::vars_os(",
            "serde(flatten)",
            "extra_body",
            "extra_headers",
        ] {
            assert!(
                !text.contains(forbidden),
                "provider config dependency or escape-hatch leak in {file}: {forbidden}"
            );
        }
    }

    let secret = production_source("src/provider/config/secret_ref.rs");
    assert_eq!(secret.matches("std::env::var(").count(), 1);
    assert!(!secret.contains("std::env::vars("));
    assert!(!secret.contains("std::env::vars_os("));

    let merge = production_source("src/provider/config/merge.rs");
    assert!(merge.contains("pub struct ProviderConfigSnapshot"));
    assert!(merge.contains("pub fn merge_layers"));
    assert!(!merge.contains("ApiKey"));
    assert!(!merge.contains("SecretString"));
}

#[test]
fn provider_registry_keeps_factory_and_runtime_snapshot_boundaries() {
    for path in [
        "src/provider/registry.rs",
        "src/provider/factory.rs",
        "src/provider/runtime.rs",
    ] {
        assert_production_file(path);
    }
    let registry = production_source("src/provider/registry.rs");
    assert!(registry.contains("Arc<RwLock<BTreeMap"));
    assert!(
        registry.contains("registration.factory.build(config, resolver)")
            || registry.contains("factory.build(config, resolver)")
    );
    assert!(registry.contains("let registration = {"));
    for forbidden in [
        "tokio::sync",
        "std::env::vars",
        "serde(flatten)",
        "extra_body",
    ] {
        assert!(
            !registry.contains(forbidden),
            "registry escape hatch: {forbidden}"
        );
    }
    let factory = production_source("src/provider/factory.rs");
    assert!(factory.contains("pub trait ProviderRuntimeFactory"));
    assert!(!factory.contains("crate::client"));
    let runtime = production_source("src/provider/runtime.rs");
    assert!(runtime.contains("pub struct ProviderRuntime"));
    assert!(!runtime.contains("RwLock"));
}

#[test]
fn auth_and_header_policy_have_bounded_owners_before_transport() {
    for path in [
        "src/provider/auth.rs",
        "src/provider/auth/providers.rs",
        "src/provider/auth/dynamic.rs",
        "src/provider/auth/cache.rs",
        "src/provider/headers.rs",
        "src/provider/headers/identity.rs",
        "src/provider/headers/dynamic.rs",
        "tests/provider_auth_contract.rs",
        "tests/provider_header_contract.rs",
    ] {
        assert_production_file(path);
    }

    for (file, text) in production_sources_under("src/provider/auth") {
        for forbidden in [
            "query_pairs_mut",
            "append_pair",
            "serde_json",
            "crate::protocol",
        ] {
            assert!(
                !text.contains(forbidden),
                "auth URL/payload/protocol escape hatch in {file}: {forbidden}"
            );
        }
    }

    let dynamic_headers = production_source("src/provider/headers/dynamic.rs");
    for forbidden in ["SecretString", "ApiKey", "AuthContext", "&mut HeaderMap"] {
        assert!(
            !dynamic_headers.contains(forbidden),
            "dynamic header secret/final-map capability: {forbidden}"
        );
    }

    let executor = production_source("src/execution/executor.rs");
    let headers = executor.find("resolve_headers_for_attempt").unwrap();
    let transport = executor.find("self.transport.execute").unwrap();
    assert!(
        headers < transport,
        "headers/auth must resolve before transport I/O"
    );
}
