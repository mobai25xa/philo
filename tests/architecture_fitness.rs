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

fn markdown_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_sources(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "md") {
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

fn root_export_names(root: &str) -> BTreeSet<String> {
    root.split("pub use ")
        .skip(1)
        .flat_map(|item| {
            let item = item.split(';').next().unwrap_or_default();
            let names = item.split_once('{').map_or_else(
                || item.rsplit("::").next().unwrap_or_default(),
                |(_, rest)| rest,
            );
            names
                .trim_end_matches('}')
                .split(',')
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
        })
        .collect()
}

fn root_export_allowlist() -> BTreeSet<String> {
    source("tests/public-root-exports.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn validate_root_exports(root: &str, allowed: &BTreeSet<String>) -> Result<(), String> {
    let exported = root_export_names(root);
    if exported != *allowed {
        return Err("crate root re-export set drifted".to_owned());
    }
    if exported.len() > 30 {
        return Err("crate root exceeds the 30-item budget".to_owned());
    }
    if root.contains("#[doc(hidden)]") || root.contains("pub mod prelude") {
        return Err("crate root hides or aliases exports".to_owned());
    }
    Ok(())
}

fn validate_core_dependencies(manifest: &str) -> Result<(), String> {
    let document = manifest
        .parse::<toml::Value>()
        .map_err(|error| format!("invalid Cargo.toml: {error}"))?;
    let dependencies = document
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing [dependencies]".to_owned())?;
    for sibling in [
        "philo-config",
        "philo-compat",
        "philo-presets",
        "philo-test-support",
    ] {
        if dependencies.contains_key(sibling) {
            return Err(format!("core production dependency includes {sibling}"));
        }
    }
    Ok(())
}

fn validate_sibling_dependencies(manifest: &str) -> Result<(), String> {
    let document = manifest
        .parse::<toml::Value>()
        .map_err(|error| format!("invalid Cargo.toml: {error}"))?;
    let dependencies = document
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing [dependencies]".to_owned())?;

    let core = dependencies
        .get("philo")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "sibling production dependencies do not point to philo".to_owned())?;
    if core.get("path").and_then(toml::Value::as_str) != Some("../..") {
        return Err("sibling philo dependency does not use the workspace core path".to_owned());
    }

    for dependency in dependencies.keys() {
        if dependency.starts_with("philo-") {
            return Err(format!(
                "sibling production dependency points sideways to {dependency}"
            ));
        }
    }
    Ok(())
}

fn validate_closed_protocol_options(module: &str) -> Result<(), String> {
    if !module.contains("pub enum ProtocolOptions") {
        return Err("ProtocolOptions is not a closed enum".to_owned());
    }
    for forbidden in [
        "Box<dyn",
        "BTreeMap<String",
        "HashMap<String",
        "serde(flatten)",
    ] {
        if module.contains(forbidden) {
            return Err(format!("ProtocolOptions uses open shape: {forbidden}"));
        }
    }
    Ok(())
}

#[test]
fn planner_is_the_only_production_history_normalization_owner() {
    let mut files = Vec::new();
    rust_sources(&crate_root().join("src"), &mut files);
    let mut owners = Vec::new();
    let mut calls = 0usize;
    for file in files {
        let relative = file.strip_prefix(crate_root()).unwrap();
        if relative == Path::new("src/domain/history/normalize.rs") {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap();
        let production = text.split("#[cfg(test)]").next().unwrap_or_default();
        let count = production
            .lines()
            .filter(|line| {
                line.contains("normalize_history_with_limits(")
                    && !line.contains("fn normalize_history_with_limits(")
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
        "RequestRunner::new",
    ] {
        assert!(
            lifecycle.contains(required),
            "missing lifecycle stage: {required}"
        );
    }

    let runner = production_source("src/execution/request_runner.rs");
    for required in ["AttemptExecutor::new", "ResponseSession::open"] {
        assert!(
            runner.contains(required),
            "missing request runner stage: {required}"
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
        for forbidden in [
            "crate::provider::",
            "crate::protocol::",
            "crate::transport::",
        ] {
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
    ] {
        assert!(
            root.join(path).is_file(),
            "missing provider profile module: {path}"
        );
    }
    for path in [
        "crates/philo-presets/src/lib.rs",
        "crates/philo-presets/src/openrouter.rs",
        "crates/philo-presets/src/deepseek.rs",
        "crates/philo-presets/src/zai.rs",
    ] {
        assert!(
            root.join(path).is_file(),
            "missing extracted preset: {path}"
        );
    }
    for path in [
        "src/provider/profiles/openrouter.rs",
        "src/provider/profiles/deepseek.rs",
        "src/provider/profiles/zai.rs",
        "src/provider/profiles/common.rs",
    ] {
        assert!(
            !root.join(path).exists(),
            "third-party preset remains in core: {path}"
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
        "src/domain/history/normalize.rs".to_owned(),
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
fn catalog_and_protocol_contract_have_single_owners_and_no_provider_brand_branches() {
    for path in [
        "src/provider/catalog/mod.rs",
        "src/provider/catalog/entry.rs",
        "src/provider/catalog/ids.rs",
        "src/provider/catalog/source.rs",
        "src/provider/catalog/merge.rs",
        "src/provider/catalog/validate.rs",
        "src/provider/protocol_contract/mod.rs",
        "src/provider/protocol_contract/profile.rs",
        "src/provider/protocol_contract/request.rs",
        "src/provider/protocol_contract/response.rs",
        "src/provider/protocol_contract/history.rs",
        "src/protocol/openai_chat/compat/mod.rs",
        "src/protocol/openai_chat/compat/request.rs",
        "src/protocol/openai_chat/compat/response.rs",
        "src/protocol/openai_chat/compat/error.rs",
        "crates/philo-compat/src/lib.rs",
        "crates/philo-compat/src/merge.rs",
    ] {
        assert_production_file(path);
    }

    assert!(
        !crate_root().join("src/provider/compat").exists(),
        "the extracted compatibility merge layer reappeared in the core"
    );

    for directory in [
        "src/provider/catalog",
        "src/provider/protocol_contract",
        "crates/philo-compat/src",
    ] {
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

    for (file, text) in production_sources_under("src") {
        for forbidden in ["CompatPatch", "resolve_compat("] {
            assert!(
                !text.contains(forbidden),
                "compatibility merge policy reappeared in the core at {file}: {forbidden}"
            );
        }
    }

    let compat_manifest = source("crates/philo-compat/Cargo.toml");
    assert!(compat_manifest.contains("path = \"../..\""));
    let core_manifest = source("Cargo.toml");
    validate_core_dependencies(&core_manifest).unwrap();

    let driver = production_source("src/protocol/openai_chat/driver.rs");
    assert!(!driver.contains("MaxOutputTokensWireFormat"));
    assert!(!driver.contains("ToolArgumentsCompat"));
}

#[test]
fn routing_detection_and_conformance_keep_bounded_owners() {
    for path in [
        "tests/provider_routing_contract.rs",
        "tests/provider_selection_contract.rs",
        "tests/provider_conformance.rs",
        "tests/support/conformance/case.rs",
        "tests/support/conformance/offline.rs",
        "tests/support/conformance/online.rs",
        "tests/support/conformance/report.rs",
        "tests/support/conformance/redaction.rs",
    ] {
        assert_production_file(path);
    }

    // FR-003: gateway routing is expressed through the bounded body axis, so no
    // production file may reintroduce a first-class routing type anywhere.
    for path in [
        "src/provider/compat/routing.rs",
        "src/protocol/openai_chat/compat/routing.rs",
    ] {
        assert!(
            !crate_root().join(path).exists(),
            "retired routing owner reappeared: {path}"
        );
    }
    for (file, text) in production_sources_under("src") {
        for forbidden in [
            "OpenRouterRouting",
            "ProviderRequestOptions",
            "ResolvedProviderRouting",
            "ProviderRoutingWire",
            "RoutingSort",
            "UpstreamId",
        ] {
            assert!(
                !text.contains(forbidden),
                "first-class gateway routing type reappeared in {file}: {forbidden}"
            );
        }
    }

    // FR-006: provider identity is declared, never inferred. The detector and
    // every type that carried a guess are gone, and no production file may
    // reintroduce endpoint-shaped inference under any name.
    assert!(
        !crate_root().join("src/provider/detection.rs").exists(),
        "retired endpoint detection owner reappeared"
    );
    for (file, text) in production_sources_under("src") {
        for forbidden in [
            "EndpointDetector",
            "EndpointDetection",
            "DetectionSuggestion",
            "DetectionExplanation",
            "DetectionConfidence",
            "NormalizedEndpointFacts",
        ] {
            assert!(
                !text.contains(forbidden),
                "endpoint inference type reappeared in {file}: {forbidden}"
            );
        }
    }
    let factory = production_source("src/provider/factory.rs");
    assert!(factory.contains("ProviderSelector"));
    assert!(
        factory.contains("ProviderSelectionSource::Undeclared"),
        "the selector must name an undeclared outcome instead of a fallback"
    );
}

#[test]
fn provider_diagnostics_are_split_by_ownership() {
    assert!(
        !crate_root().join("src/provider/diagnostics.rs").exists(),
        "provider diagnostics catch-all remains"
    );
    let provider = production_source("src/provider/mod.rs");
    assert!(!provider.contains("mod diagnostics"));

    for (file, source) in production_sources_under("src") {
        for removed in [
            "SupportStatus",
            "EffectiveSupportStatus",
            "SupportDiagnostics",
            "ProviderDiagnostics",
            "EvidenceVerification",
            "HeaderTraceEntry",
            "TraceDecision",
            "TraceOperation",
            "DetectionConfidence",
            "EndpointDetection",
            "FallbackDimension",
            "RoutingFallback",
        ] {
            assert!(
                !source.contains(removed),
                "retired diagnostics symbol remains in {file}: {removed}"
            );
        }
    }

    let catalog = production_source("src/provider/catalog/entry.rs");
    assert!(catalog.contains("pub support_status: CapabilityStatus"));
    for forbidden in [
        "enum SupportStatus",
        "EffectiveSupportStatus",
        "SupportDiagnostics",
        "EvidenceVerification",
    ] {
        assert!(!catalog.contains(forbidden));
    }

    let headers = production_source("src/provider/headers.rs");
    for removed in ["HeaderTraceEntry", "TraceDecision", "TraceOperation"] {
        assert!(!headers.contains(removed));
    }
    let observability = production_source("src/observability/trace.rs");
    assert!(observability.contains("HeadersResolved"));
    assert!(observability.contains("steps: Arc<[(HeaderName, HeaderSource, bool, bool, bool)]>"));

    let endpoint = production_source("src/provider/endpoint/origin.rs");
    let query = production_source("src/provider/endpoint/template.rs");
    assert!(endpoint.contains("impl fmt::Debug for EndpointResolutionDiagnostics"));
    assert!(query.contains("impl fmt::Debug for EndpointQueryDiagnostic"));

    let contract = source("tests/provider_diagnostics_contract.rs");
    assert!(contract.contains("support/provider-support-matrix.toml"));
    assert!(contract.contains("support/provider-support-matrix.md"));
    assert!(!contract.contains("docs/philo"));
    assert!(!contract.contains(".parent()"));
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
    let runtime_without_comments = runtime
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "profiles::",
        "OfficialOpenAiProfile",
        "TestOnlyProfile",
        "official-openai",
        "test-only",
    ] {
        assert!(
            !runtime_without_comments.contains(forbidden),
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
            "ValidatedProtocolBinding",
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
fn the_core_keeps_only_the_secret_boundary_not_a_configuration_framework() {
    // FR-005: versioned documents, layered merge, and source provenance are
    // outside the core. What stays is the reference/resolver pair, because a
    // credential the core cannot distinguish from a plain string is a leak
    // waiting to happen.
    assert_production_file("src/provider/secret.rs");
    for path in [
        "src/provider/config/mod.rs",
        "src/provider/config/schema.rs",
        "src/provider/config/source.rs",
        "src/provider/config/merge.rs",
        "src/provider/config/validate.rs",
    ] {
        assert!(
            !crate_root().join(path).exists(),
            "configuration framework reappeared in the core: {path}"
        );
    }

    let module = production_source("src/provider/mod.rs");
    assert!(!module.contains("pub mod config;"));
    assert!(module.contains("pub mod secret;"));

    for (file, text) in production_sources_under("src") {
        for forbidden in [
            "ProviderConfigSnapshot",
            "ProviderConfigLayer",
            "ProviderConfigDocument",
            "ConfigSchemaVersion",
            "FieldProvenance",
        ] {
            assert!(
                !text.contains(forbidden),
                "configuration framework type reappeared in {file}: {forbidden}"
            );
        }
    }

    let secret = production_source("src/provider/secret.rs");
    assert_eq!(secret.matches("std::env::var(").count(), 1);
    assert!(!secret.contains("std::env::vars("));
    assert!(!secret.contains("std::env::vars_os("));
    assert!(secret.contains("pub enum SecretReference"));
    assert!(secret.contains("pub trait SecretResolver"));
    for forbidden in ["crate::client", "crate::execution", "crate::transport"] {
        assert!(
            !secret.contains(forbidden),
            "secret boundary gained a downstream dependency: {forbidden}"
        );
    }

    // The extracted crate must still exist, still depend on the core, and the
    // core must not depend back on it.
    let manifest = fs::read_to_string(crate_root().join("crates/philo-config/Cargo.toml")).unwrap();
    assert!(manifest.contains("path = \"../..\""));
    let core_manifest = fs::read_to_string(crate_root().join("Cargo.toml")).unwrap();
    validate_core_dependencies(&core_manifest).unwrap();
}

#[test]
fn provider_registry_keeps_definition_and_runtime_snapshot_boundaries() {
    for path in [
        "src/provider/registry.rs",
        "src/provider/factory.rs",
        "src/provider/runtime.rs",
    ] {
        assert_production_file(path);
    }
    let registry = production_source("src/provider/registry.rs");
    assert!(registry.contains("Arc<RwLock<BTreeMap"));
    // The compiler is cloned out of the map, then run with no lock held.
    assert!(registry.contains("factory.build_deployment(deployment, resolver)"));
    assert!(registry.contains("let factory = {"));
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
    // FR-005: one construction path. The configuration-snapshot factory trait
    // and its two built-in implementations are gone.
    assert!(!factory.contains("pub trait ProviderRuntimeFactory"));
    assert!(!factory.contains("OfficialOpenAiFactory"));
    assert!(!factory.contains("OfficialAnthropicFactory"));
    assert!(factory.contains("pub struct StaticProviderFactory"));
    assert!(factory.contains("self.definition.compile(deployment, resolver)"));
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

#[test]
fn endpoint_mapping_has_one_resolver_and_typed_pre_transport_owners() {
    for path in [
        "src/provider/endpoint/mod.rs",
        "src/provider/endpoint/config.rs",
        "src/provider/endpoint/template.rs",
        "src/provider/endpoint/mapping.rs",
        "src/provider/endpoint/origin.rs",
        "src/provider/endpoint/audience.rs",
        "src/provider/endpoint/policy.rs",
        "tests/endpoint_mapping_contract.rs",
    ] {
        assert_production_file(path);
    }
    assert!(
        !crate_root().join("src/provider/endpoint.rs").exists(),
        "legacy endpoint resolver remains"
    );

    for directory in ["src/protocol", "src/transport"] {
        for (file, text) in production_sources_under(directory) {
            for forbidden in ["EndpointTemplate", "DeploymentId", "query_pairs_mut"] {
                assert!(
                    !text.contains(forbidden),
                    "endpoint mapping ownership leaked into {file}: {forbidden}"
                );
            }
        }
    }

    let executor = production_source("src/execution/executor.rs");
    let endpoint = executor.find("resolve_target_endpoint").unwrap();
    let headers = executor.find("resolve_headers_for_attempt").unwrap();
    let transport = executor.find("self.transport.execute").unwrap();
    assert!(endpoint < headers && headers < transport);

    let request = production_source("src/protocol/openai_chat/request.rs");
    assert!(request.contains("ModelBodyWireFormat::Include"));
    assert!(request.contains("ModelBodyWireFormat::Omit"));
    assert!(!request.contains("provider_id.as_str()"));
}

#[test]
fn phase_five_protocol_adapters_are_isolated_and_wire_types_remain_private() {
    for (directory, forbidden) in [
        ("src/protocol/openai_chat", "anthropic_messages"),
        ("src/protocol/anthropic_messages", "openai_chat"),
    ] {
        for (file, text) in production_sources_under(directory) {
            assert!(
                !text.contains(forbidden),
                "protocol adapter imports the other protocol in {file}: {forbidden}"
            );
        }
    }

    for facade in ["src/lib.rs", "src/protocol/mod.rs"] {
        let text = production_source(facade);
        for forbidden in [
            "MessagesRequestWire",
            "MessageStartEventWire",
            "ContentBlockStartWire",
            "ChatCompletionChunkWire",
            "ToolCallDeltaWire",
        ] {
            assert!(
                !text.contains(forbidden),
                "protocol wire type re-exported by {facade}: {forbidden}"
            );
        }
    }
}

#[test]
fn protocol_adapters_do_not_read_provider_identity_hostname_or_profile_presets() {
    for (file, text) in production_sources_under("src/protocol") {
        for forbidden in [
            "provider_id",
            "host_str(",
            "hostname",
            "CredentialAudience",
            "provider::profiles",
            "profiles::",
            "OfficialOpenAiProfile",
            "OfficialAnthropicProfile",
            "OpenRouterProfile",
            "DeepSeekProfile",
            "ZaiStandardProfile",
            "ZaiCodingProfile",
        ] {
            assert!(
                !text.contains(forbidden),
                "protocol adapter reads provider-owned identity in {file}: {forbidden}"
            );
        }
    }
}

#[test]
fn execution_and_transport_do_not_branch_on_provider_brand() {
    for directory in ["src/execution", "src/transport"] {
        for (file, text) in production_sources_under(directory) {
            let normalized = text.to_ascii_lowercase();
            for forbidden in [
                "official-openai",
                "official-anthropic",
                "openrouter",
                "deepseek",
                "zai-standard",
                "zai-coding",
                "api.openai.com",
                "api.anthropic.com",
            ] {
                assert!(
                    !normalized.contains(forbidden),
                    "shared execution branches on provider brand in {file}: {forbidden}"
                );
            }
        }
    }
}

#[test]
fn provider_definition_builder_is_the_only_production_parts_constructor() {
    let owners = production_sources_under("src/provider")
        .into_iter()
        .filter_map(|(file, text)| {
            (file != "src/provider/profiles/test_only.rs"
                && text.contains("ProviderProfile::from_parts(ProviderProfileParts {"))
            .then_some(file)
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        owners,
        BTreeSet::from(["src/provider/definition.rs".to_owned()])
    );

    for path in [
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/official_anthropic.rs",
        "crates/philo-presets/src/openrouter.rs",
        "crates/philo-presets/src/deepseek.rs",
        "crates/philo-presets/src/zai.rs",
    ] {
        let profile = production_source(path);
        assert!(
            profile.contains("ProviderDefinition"),
            "{path} skips builder"
        );
        assert!(!profile.contains("ProviderProfileParts"));
        assert!(!profile.contains("ProviderProfile::from_parts"));
    }
}

#[test]
fn core_still_has_no_sibling_production_dependency() {
    let manifest = source("Cargo.toml");
    validate_core_dependencies(&manifest).unwrap();
}

#[test]
fn sibling_crates_depend_toward_core_without_sideways_edges() {
    for sibling in [
        "philo-config",
        "philo-compat",
        "philo-presets",
        "philo-test-support",
    ] {
        let manifest = source(&format!("crates/{sibling}/Cargo.toml"));
        validate_sibling_dependencies(&manifest)
            .unwrap_or_else(|error| panic!("{sibling}: {error}"));
    }
}

#[test]
fn protocol_contract_binding_and_public_custom_api_remain_fail_closed() {
    let definition = production_source("src/provider/definition.rs");
    let profile = production_source("src/provider/profile.rs");
    let runtime = production_source("src/provider/runtime.rs");
    let binding = production_source("src/provider/protocol_contract/binding.rs");
    let policy = production_source("src/plan/policy.rs");
    for text in [&definition, &profile, &runtime, &policy] {
        assert!(!text.contains("Option<ResolvedProtocolContract>"));
        assert!(!text.contains("protocol_contract: Option"));
    }
    let definition_state = definition
        .split("pub struct ProviderDefinition {")
        .nth(1)
        .unwrap()
        .split("impl ProviderDefinition")
        .next()
        .unwrap();
    let profile_state = profile
        .split("pub struct ProviderProfile {")
        .nth(1)
        .unwrap()
        .split("/// Typed construction input")
        .next()
        .unwrap();
    let runtime_state = runtime
        .split("pub struct ProviderRuntime {")
        .nth(1)
        .unwrap()
        .split("pub(crate) struct HeaderAttemptContext")
        .next()
        .unwrap();
    for (owner, text) in [
        ("definition", definition_state),
        ("profile", profile_state),
        ("runtime", runtime_state),
    ] {
        assert!(
            text.contains("protocol: ValidatedProtocolBinding"),
            "{owner} does not carry the atomic protocol binding"
        );
        for forbidden in [
            "protocol_id:",
            "protocol_kind:",
            "dialect:",
            "protocol_contract:",
        ] {
            assert!(
                !text.contains(forbidden),
                "{owner} restored parallel protocol state: {forbidden}"
            );
        }
    }
    for required in [
        "pub(crate) struct ValidatedProtocolBinding",
        "id: ProtocolId",
        "kind: ProtocolKind",
        "dialect: ProtocolDialect",
        "contract: ResolvedProtocolContract",
        "pub(crate) fn new(",
        "protocol id, dialect, and contract do not form a supported binding",
    ] {
        assert!(
            binding.contains(required),
            "missing binding invariant: {required}"
        );
    }
    assert!(definition.contains("ValidatedProtocolBinding::new("));
    assert!(!profile.contains("match parts.dialect"));
    assert!(!runtime.contains("match profile.dialect"));
    assert!(definition.contains("CredentialBinding::exact_https_origin"));
    assert!(definition.contains("resolve_definition_endpoint(&endpoint, &catalog)"));
    assert!(definition.contains("resolve_official(endpoint)"));

    let openai = production_source("src/provider/profiles/official_openai.rs");
    let anthropic = production_source("src/provider/profiles/official_anthropic.rs");
    assert!(openai.contains("CredentialAudience::OfficialOpenAi.into()"));
    assert!(anthropic.contains("CredentialAudience::OfficialAnthropic.into()"));

    for forbidden in ["host_str()", "EndpointDetection", "detect_protocol"] {
        assert!(!runtime.contains(forbidden));
    }

    for facade in ["src/lib.rs", "src/provider/mod.rs"] {
        let text = production_source(facade);
        for forbidden in [
            "OpenAiChatDriver",
            "AnthropicMessagesDriver",
            "MessagesStateMachine",
            "ChatCompletionChunkWire",
            "MessagesRequestWire",
        ] {
            assert!(
                !text.contains(forbidden),
                "custom provider facade leaks protocol implementation in {facade}: {forbidden}"
            );
        }
    }
}

#[test]
fn runtime_and_planner_keep_concrete_protocol_policy_in_contract_owners() {
    let runtime = production_source("src/provider/runtime.rs");
    let planner = production_source("src/execution/planner.rs");
    let contract = production_source("src/provider/protocol_contract/mod.rs");

    for forbidden in [
        "ProtocolOptions::anthropic_messages",
        ".adaptive_thinking()",
        ".effort()",
        "protocol_options.anthropic.adaptive_thinking",
        "protocol_options.anthropic.effort",
    ] {
        assert!(
            !runtime.contains(forbidden),
            "runtime restored concrete protocol option policy: {forbidden}"
        );
    }
    assert!(runtime.contains("protocol.validate_options("));
    assert!(runtime.contains(".validate_capabilities("));
    assert!(!runtime.contains(".openai_chat()"));
    assert!(!planner.contains(".openai_chat()"));
    assert!(!planner.contains("CompatField::"));
    assert!(planner.contains("policy.protocol.provenance_source()"));
    for required in [
        "pub(crate) fn validate_options(",
        "pub(crate) fn validate_capabilities(",
        "fn validate_options(",
        "pub(crate) fn provenance_source(",
        "protocol_options.anthropic.adaptive_thinking",
        "protocol_options.anthropic.effort",
    ] {
        assert!(
            contract.contains(required),
            "missing contract policy: {required}"
        );
    }
}

#[test]
fn sse_machine_polling_has_one_protocol_private_owner() {
    let protocol = production_source("src/protocol/mod.rs");
    let shared = production_source("src/protocol/response_stream.rs");
    let openai = production_source("src/protocol/openai_chat/response/stream.rs");
    let anthropic = production_source("src/protocol/anthropic_messages/response/stream.rs");
    let transport = production_source("src/transport/sse.rs");

    assert!(protocol.contains("mod response_stream;"));
    assert!(!protocol.contains("pub(crate) mod response_stream;"));
    for required in [
        "pub(crate) trait SseEventMachine",
        "pub(crate) struct SseMachineStream<M>",
        "impl<M> Stream for SseMachineStream<M>",
        "stream.pending.pop_front()",
        "stream.max_events_per_poll",
        "stream.machine.finish()",
        "error.into_llm_error()",
    ] {
        assert!(
            shared.contains(required),
            "missing shared SSE rule: {required}"
        );
    }
    for (name, wrapper) in [("openai", openai), ("anthropic", anthropic)] {
        assert!(wrapper.contains("impl SseEventMachine for"));
        assert!(wrapper.contains("SseMachineStream::new("));
        for forbidden in [
            "fn poll_next(",
            "VecDeque",
            "max_events_per_poll:",
            "terminal:",
            "SseDecoder",
        ] {
            assert!(
                !wrapper.contains(forbidden),
                "{name} wrapper restored shared SSE mechanism: {forbidden}"
            );
        }
    }
    for forbidden in [
        "AssistantEvent",
        "SseEventMachine",
        "ChatStateMachine",
        "MessagesStateMachine",
    ] {
        assert!(
            !transport.contains(forbidden),
            "transport imported protocol/domain semantics: {forbidden}"
        );
    }
}

#[test]
fn driver_preparation_mechanisms_have_one_protocol_private_owner() {
    let protocol = production_source("src/protocol/mod.rs");
    let shared = production_source("src/protocol/preparation.rs");
    let openai = production_source("src/protocol/openai_chat/driver.rs");
    let anthropic = production_source("src/protocol/anthropic_messages/driver.rs");

    assert!(protocol.contains("mod preparation;"));
    assert!(!protocol.contains("pub(crate) mod preparation;"));
    for required in [
        "struct CommonRequestFacts",
        "fn scan(request: &PlannedRequest)",
        "fn standard_json_sse_header_operations()",
        "reasoning_requested",
        "max_output_tokens_requested",
    ] {
        assert!(
            shared.contains(required),
            "missing preparation owner: {required}"
        );
    }
    for forbidden in [
        "OpenAi",
        "Anthropic",
        "ProviderId",
        "ProductId",
        "Wire",
        "Transport",
    ] {
        assert!(
            !shared.contains(forbidden),
            "shared preparation helper gained concrete authority: {forbidden}"
        );
    }
    for (name, driver) in [("openai", openai), ("anthropic", anthropic)] {
        for required in [
            "CommonRequestFacts::scan(&plan.planned)",
            "standard_json_sse_header_operations()",
            "HttpResponseRequirements::event_stream(",
            "ProtocolFactDecisions",
        ] {
            assert!(
                driver.contains(required),
                "{name} misses shared primitive: {required}"
            );
        }
        for forbidden in [
            "header::CONTENT_TYPE",
            "header::ACCEPT",
            "ContentPart::Image",
            "MessageRole::Tool",
        ] {
            assert!(
                !driver.contains(forbidden),
                "{name} restored shared preparation mechanism: {forbidden}"
            );
        }
    }
}

#[test]
fn structured_terminal_mechanism_is_shared_but_terminal_recognition_stays_protocol_owned() {
    let protocol = production_source("src/protocol/mod.rs");
    let shared = production_source("src/protocol/structured_terminal.rs");
    let openai = production_source("src/protocol/openai_chat/response/machine.rs");
    let anthropic = production_source("src/protocol/anthropic_messages/response/machine.rs");
    let collector = production_source("src/domain/event.rs");

    assert!(protocol.contains("mod structured_terminal;"));
    assert!(!protocol.contains("pub(crate) mod structured_terminal;"));
    for required in [
        "struct StructuredTerminal",
        "push_answer_text",
        "validate_before_done",
        "is_validated",
        "text_buffer: Option<String>",
        "schema_limits: SchemaLimits",
        "byte_limit: usize",
    ] {
        assert!(
            shared.contains(required),
            "missing terminal mechanism: {required}"
        );
    }
    for (name, machine, marker) in [
        ("openai", openai, "event.data() == \"[DONE]\""),
        ("anthropic", anthropic, "fn message_stop("),
    ] {
        assert!(machine.contains("StructuredTerminal::new("));
        assert!(machine.contains("validate_before_done("));
        assert!(
            machine.contains(marker),
            "{name} lost protocol terminal recognition"
        );
        assert!(!machine.contains("validate_structured_response("));
    }
    assert!(collector.contains("validate_structured_output(&message, response_format)?"));
    assert!(collector.contains("validate_structured_response("));
}

#[test]
fn provider_product_runtime_registry_identity_is_exact_and_fail_closed() {
    let registry = production_source("src/provider/registry.rs");
    let error = production_source("src/error.rs");
    let selector = production_source("src/provider/factory.rs");

    for required in [
        "ProviderRegistration::from_definition",
        "ProviderRegistryFailure::AmbiguousProductSelection",
        "select one exact product",
        "Some(runtime.product_id()) != metadata.product_id.as_ref()",
        "Some(runtime.protocol_id()) != metadata.protocol_id.as_ref()",
        "registration.compiler.as_ref().clone()",
    ] {
        assert!(
            registry.contains(required),
            "registry lost exact identity rule: {required}"
        );
    }
    assert!(error.contains("AmbiguousProductSelection"));
    assert!(selector.contains("Every source is explicit"));
    for forbidden in ["host_str()", "EndpointDetection", "detect_provider"] {
        assert!(
            !selector.contains(forbidden),
            "provider selector restored inference: {forbidden}"
        );
    }

    for path in [
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/official_anthropic.rs",
        "crates/philo-presets/src/openrouter.rs",
        "crates/philo-presets/src/deepseek.rs",
        "crates/philo-presets/src/zai.rs",
    ] {
        let source = production_source(path);
        assert!(
            source.contains("ProviderDefinition"),
            "{path} bypasses the unique definition builder path"
        );
    }
}

#[test]
fn responses_readiness_has_extension_slots_without_placeholder_support() {
    let kind = production_source("src/plan/policy.rs");
    let dispatch = production_source("src/protocol/mod.rs");
    let binding = production_source("src/provider/protocol_contract/binding.rs");
    let contracts = production_source("src/provider/protocol_contract/mod.rs");
    let options = production_source("src/protocol_options/mod.rs");
    let shared_stream = production_source("src/protocol/response_stream.rs");
    let source_identity = production_source("src/domain/content.rs");

    for (owner, source, required) in [
        ("plan", &kind, "enum ProtocolKind"),
        ("dispatch", &dispatch, "enum ProtocolDispatch"),
        ("operation", &dispatch, "enum ProtocolOperation"),
        ("response", &dispatch, "enum ProtocolResponsePlan"),
        ("binding", &binding, "struct ValidatedProtocolBinding"),
        ("contract", &contracts, "enum ResolvedProtocolContract"),
        ("options", &options, "enum ProtocolOptions"),
    ] {
        assert!(
            source.contains(required),
            "missing third-protocol extension slot in {owner}: {required}"
        );
    }
    assert!(dispatch.contains("mod response_stream;"));
    assert!(shared_stream.contains("struct SseMachineStream"));
    validate_closed_protocol_options(&options).unwrap();

    for (file, text) in production_sources_under("src") {
        for forbidden in [
            "OpenAiResponses",
            "openai_responses",
            "PreviousResponseId",
            "previous_response_id",
            "ContinuationState",
            "BackgroundJob",
            "FileUpload",
            "HostedTool",
        ] {
            assert!(
                !text.contains(forbidden),
                "{file} claims deferred Responses support through {forbidden}"
            );
        }
    }

    let identity_state = source_identity
        .split("pub struct SourceIdentity {")
        .nth(1)
        .unwrap()
        .split("impl SourceIdentity")
        .next()
        .unwrap();
    assert!(source_identity.contains("This is not conversation or response continuation state"));
    assert!(
        source_identity.contains("This comparison does not authorize conversation continuation")
    );
    assert!(identity_state.contains("provider: ProviderId"));
    assert!(identity_state.contains("protocol: ProtocolId"));
    assert!(!identity_state.contains("ProductId"));
    assert!(!identity_state.contains("continuation"));
}

#[test]
fn phase_five_adapters_have_no_network_or_tool_execution_authority() {
    for (file, text) in production_sources_under("src/protocol") {
        for forbidden in [
            "reqwest::Client",
            "TcpStream",
            "UdpSocket",
            "std::process::Command",
            "tokio::process::Command",
            "execute_tool",
            "invoke_tool",
        ] {
            assert!(
                !text.contains(forbidden),
                "protocol adapter gained I/O or tool execution authority in {file}: {forbidden}"
            );
        }
    }
}

#[test]
fn phase_five_runtime_reliability_and_complete_remain_protocol_neutral() {
    for directory in ["src/execution", "src/client"] {
        for (file, text) in production_sources_under(directory) {
            for forbidden in [
                "anthropic-messages",
                "openai-chat-completions",
                "ProtocolDialect::AnthropicMessages",
                "ProtocolDialect::OpenAiChatCompletions",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "shared lifecycle branches on protocol in {file}: {forbidden}"
                );
            }
        }
    }

    let client = production_source("src/client/lifecycle.rs");
    let complete = client.find("pub async fn complete(").unwrap();
    let complete_body = &client[complete..];
    assert!(complete_body.contains("self.stream(request).await?"));
    assert!(complete_body.contains("collect_assistant_message_for_format"));
}

#[test]
fn phase_five_official_profiles_use_shared_runtime_pipelines() {
    for path in [
        "src/provider/profiles/official_openai.rs",
        "src/provider/profiles/official_anthropic.rs",
    ] {
        let profile = production_source(path);
        for required in [
            "ProviderDefinition",
            "compile_resolved",
            "CredentialAudience",
        ] {
            assert!(
                profile.contains(required),
                "{path} bypasses shared owner: {required}"
            );
        }
        for forbidden in [
            "ProviderProfile::from_parts",
            "ProviderProfileParts",
            "reqwest::Client",
            "MockTransport",
            "LlmClient::new",
        ] {
            assert!(
                !profile.contains(forbidden),
                "official profile creates execution/transport state in {path}: {forbidden}"
            );
        }
    }

    for path in [
        "crates/philo-presets/src/openrouter.rs",
        "crates/philo-presets/src/deepseek.rs",
        "crates/philo-presets/src/zai.rs",
    ] {
        let preset = production_source(path);
        assert!(preset.contains("ProviderDefinition"));
        assert!(preset.contains("definition.compile(&deployment"));
        assert!(!preset.contains("compile_resolved"));
    }
}

#[test]
fn history_policy_contains_legality_only_and_resource_limits_fail_closed_elsewhere() {
    let policy = production_source("src/domain/history/policy.rs");
    assert!(!policy.contains("max_messages"));
    assert!(!policy.contains("max_total_text_bytes"));
    assert!(!policy.contains("SynthesizeError"));
    assert!(!policy.contains("Defer"));
    assert!(policy.contains("DropWithDiagnostic"));

    let normalize = production_source("src/domain/history/normalize.rs");
    assert!(normalize.contains("normalize_history_with_limits"));
    assert!(normalize.contains("HistoryFailure::TooManyMessages"));
    assert!(normalize.contains("HistoryFailure::TextTooLarge"));
}

#[test]
fn call_plan_has_one_private_owner() {
    let root = crate_root();
    for path in [
        "src/plan/mod.rs",
        "src/plan/contract.rs",
        "src/plan/policy.rs",
    ] {
        assert!(root.join(path).is_file(), "missing plan owner: {path}");
    }
    for path in ["src/provider/call_policy.rs", "src/execution/contract.rs"] {
        assert!(
            !root.join(path).exists(),
            "legacy plan owner remains: {path}"
        );
    }
    let protocol = production_source("src/protocol/mod.rs");
    assert!(protocol.contains("crate::plan::"));
    assert!(!protocol.contains("crate::execution::"));
    assert!(!production_source("src/lib.rs").contains("pub mod plan"));
}

#[test]
fn phase_five_protocol_accumulators_use_typed_resource_limits() {
    let anthropic_request = production_source("src/protocol/anthropic_messages/request.rs");
    let anthropic_history = production_source("src/protocol/anthropic_messages/history.rs");
    let anthropic_machine =
        production_source("src/protocol/anthropic_messages/response/machine.rs");
    let anthropic_stream = production_source("src/protocol/anthropic_messages/response/stream.rs");
    assert!(
        anthropic_request.contains("max_body_bytes"),
        "missing request body limit"
    );
    for required in [
        "MAX_STREAM_EVENTS",
        "max_tool_arguments_bytes",
        "max_structured_output_bytes",
        "MAX_OPAQUE_THINKING_BYTES",
    ] {
        assert!(
            anthropic_machine.contains(required),
            "missing decoder limit: {required}"
        );
    }
    for required in ["max_inline_image_bytes", "max_image_url_bytes"] {
        assert!(
            anthropic_history.contains(required),
            "missing history limit: {required}"
        );
    }
    assert!(anthropic_stream.contains("SseConfig"));
    assert!(anthropic_stream.contains("ResponseLimits"));
}

#[test]
fn the_four_extension_axes_share_one_protection_table_owner() {
    assert_production_file("src/protected.rs");

    let owner = production_source("src/protected.rs");
    for required in [
        "PROTECTED_HEADERS",
        "AUTH_INELIGIBLE_HEADERS",
        "PROTECTED_BODY_KEY_SHAPES",
        "ANTHROPIC_MESSAGES_PROTECTED_BODY_FIELDS",
        "OPENAI_CHAT_PROTECTED_BODY_FIELDS",
        "REQUIRED_ENDPOINT_SCHEME",
    ] {
        assert!(
            owner.contains(required),
            "missing protection table: {required}"
        );
    }

    // No axis may keep a private copy of the decision. A file may still name headers
    // for a different rule — `HeaderPolicy::allows` routes each one to its owner — but
    // if it names a protected header it must take the protection verdict from here.
    for (file, text) in production_sources_under("src") {
        if file == "src/protected.rs" || !text.contains("\"proxy-authorization\"") {
            continue;
        }
        assert!(
            text.contains("crate::protected::"),
            "{file} names protected headers without deferring to the single owner"
        );
    }
}

#[test]
fn both_protocols_share_one_bounded_raw_body_extension_implementation() {
    for path in [
        "src/protocol_options/mod.rs",
        "src/protocol_options/raw.rs",
        "src/protocol_options/anthropic.rs",
        "src/protocol_options/openai.rs",
    ] {
        assert_production_file(path);
    }
    assert!(
        !crate_root().join("src/extensions.rs").exists(),
        "legacy extensions.rs remains"
    );

    // The budget and shape rules exist once, in `raw.rs`.
    let raw = production_source("src/protocol_options/raw.rs");
    for required in [
        "MAX_RAW_BYTES",
        "MAX_RAW_KEYS",
        "MAX_RAW_ARRAY_ITEMS",
        "MAX_RAW_DEPTH",
        "MAX_RAW_KEY_BYTES",
    ] {
        assert!(raw.contains(required), "missing raw budget: {required}");
    }
    for protocol in ["anthropic", "openai"] {
        let text = production_source(&format!("src/protocol_options/{protocol}.rs"));
        assert!(
            text.contains("RawFields::parse"),
            "{protocol} raw extension does not reuse the shared core"
        );
        assert!(
            text.contains("dangerous_from_object"),
            "{protocol} raw extension drops the explicit dangerous name"
        );
        for forbidden in ["MAX_RAW_BYTES", "MAX_RAW_DEPTH", "fn validate_raw_value"] {
            assert!(
                !text.contains(forbidden),
                "{protocol} raw extension re-implements the bounded core: {forbidden}"
            );
        }
    }

    // The container stays a closed protocol-keyed enum.
    let module = production_source("src/protocol_options/mod.rs");
    validate_closed_protocol_options(&module).unwrap();
}

#[test]
fn the_crate_root_exports_only_the_first_request_whitelist() {
    let root = production_source("src/lib.rs");
    validate_root_exports(&root, &root_export_allowlist()).unwrap();
}

#[test]
fn deliberate_extra_root_export_is_rejected() {
    let mut mutated = production_source("src/lib.rs");
    mutated.push_str("\npub use domain::request::CapabilityStatus;\n");
    assert!(validate_root_exports(&mutated, &root_export_allowlist()).is_err());
}

#[test]
fn deliberate_sibling_dependency_is_rejected() {
    let mutated = "[dependencies]\nphilo-config = { path = \"crates/philo-config\" }\n";
    assert!(validate_core_dependencies(mutated).is_err());
}

#[test]
fn deliberate_sideways_sibling_dependency_is_rejected() {
    let mutated = concat!(
        "[dependencies]\n",
        "philo = { path = \"../..\" }\n",
        "philo-compat = { path = \"../philo-compat\" }\n",
    );
    assert!(validate_sibling_dependencies(mutated).is_err());
}

#[test]
fn deliberate_open_protocol_options_container_is_rejected() {
    let mutated = "pub enum ProtocolOptions { Custom(HashMap<String, serde_json::Value>) }";
    assert!(validate_closed_protocol_options(mutated).is_err());
}

#[test]
fn current_user_docs_do_not_reintroduce_removed_rust_paths() {
    let mut files = vec![crate_root().join("README.md")];
    markdown_sources(&crate_root().join("docs"), &mut files);
    files.sort();

    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for removed in [
            "philo::provider::config",
            "philo::provider::compat",
            "philo::provider::detection",
            "philo::provider::diagnostics",
        ] {
            assert!(
                !text.contains(removed),
                "removed Rust path returned in {}: {removed}",
                file.display()
            );
        }
    }
}
