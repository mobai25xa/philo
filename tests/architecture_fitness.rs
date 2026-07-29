//! Stable architecture boundaries that are not covered by behavioral tests.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn rust_sources(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, directory: &Path, output: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, output);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = fs::read_to_string(path)
                    .unwrap()
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                output.push((relative, source));
            }
        }
    }

    let repository = root();
    let mut output = Vec::new();
    visit(&repository, &repository.join(relative), &mut output);
    output.sort_by(|left, right| left.0.cmp(&right.0));
    output
}

fn root_exports(source: &str) -> BTreeSet<String> {
    source
        .split("pub use ")
        .skip(1)
        .flat_map(|item| {
            let declaration = item.split(';').next().unwrap_or_default();
            let names = declaration.split_once('{').map_or_else(
                || declaration.rsplit("::").next().unwrap_or_default(),
                |(_, names)| names,
            );
            names
                .trim_end_matches('}')
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn root_export_allowlist() -> BTreeSet<String> {
    read("tests/public-root-exports.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn dependency_table(manifest: &toml::Value) -> &toml::map::Map<String, toml::Value> {
    manifest["dependencies"]
        .as_table()
        .expect("manifest must define [dependencies]")
}

#[test]
fn workspace_contains_only_the_three_maintained_packages() {
    let manifest: toml::Value = toml::from_str(&read("Cargo.toml")).unwrap();
    let members = manifest["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        members,
        BTreeSet::from(["crates/philo-config", "crates/philo-presets"])
    );
    assert!(!root().join("crates/philo-compat").exists());
    assert!(!root().join("crates/philo-test-support").exists());

    for sibling in ["philo-config", "philo-presets"] {
        assert!(
            root()
                .join("crates")
                .join(sibling)
                .join("Cargo.toml")
                .is_file()
        );
    }
}

#[test]
fn production_dependencies_point_toward_core_without_sideways_edges() {
    let core: toml::Value = toml::from_str(&read("Cargo.toml")).unwrap();
    for sibling in ["philo-config", "philo-presets"] {
        assert!(!dependency_table(&core).contains_key(sibling));
    }

    for sibling in ["philo-config", "philo-presets"] {
        let path = format!("crates/{sibling}/Cargo.toml");
        let manifest: toml::Value = toml::from_str(&read(&path)).unwrap();
        let dependencies = dependency_table(&manifest);
        assert_eq!(
            dependencies["philo"]["path"].as_str(),
            Some("../.."),
            "{sibling} must depend on the workspace core"
        );
        assert!(
            dependencies
                .keys()
                .all(|dependency| !dependency.starts_with("philo-") || dependency == "philo"),
            "{sibling} has a sideways package dependency"
        );
    }
}

#[test]
fn domain_remains_provider_protocol_transport_and_network_independent() {
    for (path, source) in rust_sources("src/domain") {
        for forbidden in [
            "crate::provider::",
            "crate::protocol::",
            "crate::transport::",
            "reqwest::",
            "tokio::net",
        ] {
            assert!(!source.contains(forbidden), "{path} depends on {forbidden}");
        }
    }
}

#[test]
fn protocol_and_execution_have_no_network_or_provider_brand_authority() {
    for (path, source) in rust_sources("src/protocol") {
        for forbidden in [
            "reqwest::",
            "tokio::net",
            "TcpStream",
            "OfficialOpenAiProfile",
        ] {
            assert!(!source.contains(forbidden), "{path} contains {forbidden}");
        }
    }
    for (path, source) in rust_sources("src/execution") {
        for forbidden in [
            "OfficialOpenAiProfile",
            "OfficialAnthropicProfile",
            "OpenRouterProfile",
            "DeepSeekProfile",
            "ZaiStandardProfile",
            "ZaiCodingProfile",
        ] {
            assert!(!source.contains(forbidden), "{path} contains {forbidden}");
        }
    }
}

#[test]
fn protocol_options_are_closed_and_wire_implementations_stay_private() {
    let options = read("src/protocol_options/mod.rs");
    assert!(options.contains("pub enum ProtocolOptions"));
    for forbidden in [
        "Box<dyn",
        "BTreeMap<String",
        "HashMap<String",
        "serde(flatten)",
    ] {
        assert!(
            !options.contains(forbidden),
            "open protocol option shape: {forbidden}"
        );
    }

    let library = read("src/lib.rs");
    for forbidden in [
        "OpenAiChatDriver",
        "AnthropicMessagesDriver",
        "ResponseSession",
    ] {
        assert!(!root_exports(&library).contains(forbidden));
    }
}

#[test]
fn crate_root_exports_exactly_the_reviewed_request_surface() {
    let library = read("src/lib.rs");
    assert_eq!(root_exports(&library), root_export_allowlist());
    assert!(!library.contains("#[doc(hidden)]"));
    assert!(!library.contains("pub mod prelude"));
    for removed in ["TestOnlyProfile", "MockTransport", "PHASE_", "test_util"] {
        assert!(
            !library.contains(removed),
            "removed public surface returned: {removed}"
        );
    }
}

#[test]
fn stable_cross_layer_owners_exist_once() {
    for owner in [
        "src/execution/planner.rs",
        "src/execution/request_runner.rs",
        "src/protocol/preparation.rs",
        "src/protocol/response.rs",
        "src/protocol/structured_terminal.rs",
        "src/provider/definition.rs",
        "src/provider/endpoint/mapping.rs",
        "src/provider/protocol_contract/mod.rs",
        "src/transport/network.rs",
    ] {
        assert!(
            root().join(owner).is_file(),
            "missing stable owner: {owner}"
        );
    }
}

#[test]
fn temporary_packages_and_public_paths_do_not_return() {
    for document in ["README.md", "COMPATIBILITY.md", "SECURITY.md"] {
        let content = read(document);
        for retired in ["philo_compat", "philo_test_support", "TestOnlyProfile"] {
            assert!(
                !content.contains(retired),
                "{document} references {retired}"
            );
        }
    }
}
