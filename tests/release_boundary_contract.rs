//! Executable contracts for the candidate release boundary and compatibility policy.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct BehaviorRegistry {
    schema_version: u32,
    review_policy: String,
    allowed_change_classes: Vec<String>,
    contract: Vec<BehaviorContract>,
}

#[derive(Deserialize)]
struct BehaviorContract {
    id: String,
    owner: String,
    test_files: Vec<String>,
    golden_roots: Vec<String>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

#[test]
fn behavior_contracts_have_one_owner_and_live_evidence() {
    let registry: BehaviorRegistry =
        toml::from_str(&read("support/behavior-contracts.toml")).unwrap();
    assert_eq!(registry.schema_version, 1);
    assert!(registry.review_policy.contains("Compatible"));
    assert!(
        registry
            .review_policy
            .contains("change-approval-template.md")
    );
    assert_eq!(
        registry.allowed_change_classes,
        ["Compatible", "Bug Fix", "Breaking", "External Drift"]
    );

    let mut ids = BTreeSet::new();
    let mut owners = BTreeSet::new();
    for contract in registry.contract {
        assert!(
            ids.insert(contract.id.clone()),
            "duplicate contract {}",
            contract.id
        );
        assert!(!contract.owner.trim().is_empty());
        owners.insert(contract.owner);
        assert!(
            !contract.test_files.is_empty(),
            "{} has no tests",
            contract.id
        );
        assert!(
            !contract.golden_roots.is_empty(),
            "{} has no golden owner",
            contract.id
        );
        for path in contract.test_files {
            assert!(root().join(&path).is_file(), "missing behavior test {path}");
        }
        for path in contract.golden_roots {
            assert!(root().join(&path).is_dir(), "missing golden root {path}");
        }
    }
    assert!(owners.len() >= 6, "behavior ownership is over-centralized");
}

#[test]
fn compatibility_gate_and_examples_are_explicit_ci_steps() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(workflow.contains("tools/check-api-compatibility.ps1"));
    assert!(workflow.contains("cargo check --examples"));
    assert!(workflow.contains("consumers/stable/Cargo.toml"));
    assert!(workflow.contains("consumers/experimental/Cargo.toml"));
    assert!(!root().join("tests/migration_public_paths.rs").exists());
    assert!(!root().join("tests/examples_compile_contract.rs").exists());
}

#[test]
fn every_top_level_integration_test_has_exactly_one_capability_owner() {
    let registry: toml::Value = toml::from_str(&read("support/test-ownership.toml")).unwrap();
    let capabilities = registry["capability"].as_array().unwrap();
    let mut owned = BTreeSet::new();
    for capability in capabilities {
        let name = capability["name"].as_str().unwrap();
        for test in capability["tests"].as_array().unwrap() {
            let path = test.as_str().unwrap();
            assert!(
                root().join(path).is_file(),
                "{name} references missing test {path}"
            );
            if Path::new(path).parent() == Some(Path::new("tests")) {
                assert!(
                    owned.insert(path.to_owned()),
                    "test has multiple owners: {path}"
                );
            }
        }
    }

    let actual = fs::read_dir(root().join("tests"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| {
            format!(
                "tests/{}",
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(owned, actual, "test ownership must match top-level entries");
}

#[test]
fn capability_inventory_has_one_explicit_release_decision_per_package_and_feature() {
    let inventory: toml::Value =
        toml::from_str(&read("docs/maintenance/capability-inventory.toml"))
            .expect("capability inventory must be valid TOML");

    assert_eq!(inventory["schema_version"].as_integer(), Some(1));
    assert_eq!(inventory["api_baseline_created"].as_bool(), Some(false));

    let packages = inventory["package"]
        .as_array()
        .expect("package decisions must be an array");
    let package_names = packages
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().expect("package name");
            assert!(
                matches!(entry["decision"].as_str(), Some("Stable" | "Experimental")),
                "invalid package decision for {name}"
            );
            for key in ["publish", "version_policy", "owner", "notes"] {
                assert!(
                    entry[key].as_str().is_some_and(|value| !value.is_empty()),
                    "{name} is missing {key}"
                );
            }
            name
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        package_names,
        BTreeSet::from(["philo", "philo-config", "philo-presets"])
    );

    let features = inventory["feature"]
        .as_array()
        .expect("feature decisions must be an array");
    let feature_names = features
        .iter()
        .map(|entry| entry["name"].as_str().expect("feature name"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        feature_names,
        BTreeSet::from(["default", "rustls-tls", "tracing"])
    );
}

#[test]
fn every_capability_has_a_level_owner_test_documentation_and_boundary() {
    let inventory: toml::Value =
        toml::from_str(&read("docs/maintenance/capability-inventory.toml"))
            .expect("capability inventory must be valid TOML");
    let capabilities = inventory["capability"]
        .as_array()
        .expect("capabilities must be an array");
    assert!(capabilities.len() >= 10);

    let mut ids = BTreeSet::new();
    for capability in capabilities {
        let id = capability["id"].as_str().expect("capability id");
        assert!(ids.insert(id), "duplicate capability id: {id}");
        assert!(matches!(
            capability["stability"].as_str(),
            Some("Stable" | "Experimental" | "Escape Hatch")
        ));
        assert!(
            capability["public_entry_points"]
                .as_array()
                .is_some_and(|values| !values.is_empty()),
            "{id} has no public entry point"
        );
        for key in [
            "behavior_owner",
            "test_owner",
            "documentation_owner",
            "security_boundary",
            "external_verification",
            "decision",
        ] {
            assert!(
                capability[key]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "{id} is missing {key}"
            );
        }
    }
}

#[test]
fn compatibility_policy_covers_required_change_and_release_paths() {
    let policy = read("COMPATIBILITY.md");
    for heading in [
        "## Stability classes",
        "## Version and package policy",
        "## SemVer decision matrix",
        "## Behavior compatibility",
        "## MSRV policy",
        "## Deprecation policy",
        "## Change approval",
        "## Hotfix, backport, and yank",
    ] {
        assert!(
            policy.contains(heading),
            "missing policy section: {heading}"
        );
    }
    for required in [
        "Rust `1.97.1`",
        "two Minor releases",
        "six months",
        "Security and Release owner approval",
        "Provider-side drift",
        "default feature",
        "closed public enum",
    ] {
        assert!(policy.contains(required), "missing policy rule: {required}");
    }
}

#[test]
fn public_metadata_uses_capability_names_not_stage_names() {
    let library = read("src/lib.rs");
    assert!(library.contains("pub const OPENAI_CHAT_CONTRACT_ID"));
    assert!(library.contains("pub const RELIABILITY_CONTRACT_ID"));

    let readme = read("README.md");
    assert!(readme.contains("OpenAI Chat Completions"));
    assert!(readme.contains("Anthropic Messages"));
}

#[test]
fn release_package_excludes_repository_only_assets() {
    let manifest: toml::Value =
        toml::from_str(&read("Cargo.toml")).expect("workspace manifest must be valid TOML");
    let excludes = manifest["package"]["exclude"]
        .as_array()
        .expect("core package must have an explicit exclude list")
        .iter()
        .map(|value| value.as_str().expect("exclude entries must be strings"))
        .collect::<BTreeSet<_>>();
    for required in [
        ".gitattributes",
        ".github/**",
        ".gitignore",
        "benches/**",
        "compatibility/**",
        "deny.toml",
        "docs/**",
        "rust-toolchain.toml",
        "rustfmt.toml",
        "support/**",
        "tests/**",
        "tools/**",
        "src/client/http_e2e_tests.rs",
        "src/client/release_tests.rs",
        "src/provider/profiles/test_only.rs",
        "src/transport/contract_tests.rs",
    ] {
        assert!(
            excludes.contains(required),
            "missing package exclude: {required}"
        );
    }
}

#[test]
fn release_workflow_enforces_one_candidate_and_one_manifest() {
    let workflow = read(".github/workflows/release.yml");
    for required in [
        "environment: release-candidate",
        "environment: stable-release",
        "if: inputs.mode == 'publish'",
        "head_sha",
        "performance-$PHILO_CANDIDATE_SHA-release",
        "canary-${{ inputs.subject_commit }}-official-openai",
        "canary-${{ inputs.subject_commit }}-official-anthropic",
        "release-manifest.json",
        "cargo publish -p philo --locked --dry-run",
        "cargo publish -p philo --locked",
        "philo-v$PHILO_VERSION",
    ] {
        assert!(
            workflow.contains(required),
            "missing release gate: {required}"
        );
    }
    assert!(!workflow.contains("pull_request_target"));
    assert!(!workflow.contains("uses: actions/checkout@v"));
    assert!(!workflow.contains("uses: actions/upload-artifact@v"));

    let builder = read("tools/build-release-evidence.ps1");
    assert!(builder.contains("philo/release-manifest"));
    assert!(builder.contains("SPDX-2.3"));
    assert!(builder.contains("API and Release reviewers must be distinct"));
    assert_eq!(builder.matches("release-manifest.json").count(), 1);
}
