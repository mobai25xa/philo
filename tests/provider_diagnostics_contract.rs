//! Catalog evidence and structured support-matrix contracts.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use philo::ModelId;
use philo::domain::request::CapabilityStatus;
use philo_presets::DeepSeekProfile;
use serde::Deserialize;

const CREDENTIAL_CANARY: &str = "diagnostics-credential-canary";

#[derive(Debug, Deserialize)]
struct Matrix {
    schema_version: u32,
    generated_as_of: String,
    status_vocabulary: Vec<String>,
    evidence_level_vocabulary: Vec<String>,
    official_protocols: Vec<OfficialProtocol>,
    entries: Vec<MatrixEntry>,
}

#[derive(Debug, Deserialize)]
struct OfficialProtocol {
    provider_id: String,
    product_id: String,
    protocol_id: String,
    stability_level: String,
    exact_models: Vec<String>,
    evidence_source: String,
    offline_conformance: String,
    last_canary_attempt: String,
    last_canary_success: String,
    reviewed_at: String,
    expires_at: String,
    owner: String,
    removal_policy: String,
    known_limitations: Vec<String>,
    capabilities: Vec<OfficialCapability>,
}

#[derive(Debug, Deserialize)]
struct OfficialCapability {
    case: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct MatrixEntry {
    provider_id: String,
    product_id: String,
    protocol_id: String,
    stability_level: String,
    workflow_id: String,
    owner: String,
    exact_model: String,
    profile_version: String,
    contract_version: String,
    catalog_version: String,
    compat_version: String,
    catalog_status: String,
    effective_status: String,
    evidence_levels: Vec<String>,
    offline_evidence_id: String,
    fixture_manifest: String,
    evidence_source: String,
    candidate_sha: String,
    run_url: String,
    online_status: String,
    reviewed_at: String,
    expires_at: String,
    last_canary_attempt: String,
    last_canary_success: String,
    removal_policy: String,
    limitations: Vec<String>,
    capabilities: Vec<MatrixCapability>,
}

#[derive(Debug, Deserialize)]
struct MatrixCapability {
    case: String,
    status: String,
    evidence_id: String,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn matrix_path() -> PathBuf {
    repository_root().join("support/provider-support-matrix.toml")
}

fn load_matrix() -> Matrix {
    let text = fs::read_to_string(matrix_path()).expect("read support matrix");
    toml::from_str(&text).expect("parse support matrix")
}

fn assert_hosted_evidence(entry: &MatrixEntry, key: &str) {
    assert_eq!(
        entry.candidate_sha.len(),
        40,
        "{key} candidate_sha must be exact 40-char SHA"
    );
    assert!(
        entry
            .candidate_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()),
        "{key} candidate_sha must be hex"
    );
    assert!(
        entry.run_url.starts_with("https://github.com/"),
        "{key} run_url must be a hosted Actions URL"
    );
    assert!(
        entry
            .limitations
            .iter()
            .any(|item| item == "hosted-protected-online"),
        "{key} must record hosted-protected-online limitation tag"
    );
}

fn validate_official_protocols(matrix: &Matrix) {
    assert_eq!(matrix.official_protocols.len(), 2);
    let mut official_keys = BTreeSet::new();
    for protocol in &matrix.official_protocols {
        let key = format!("{}/{}", protocol.provider_id, protocol.product_id);
        assert!(official_keys.insert(key));
        assert!(matches!(
            protocol.stability_level.as_str(),
            "Stable" | "Experimental"
        ));
        assert!(matches!(
            protocol.protocol_id.as_str(),
            "openai-chat-completions" | "anthropic-messages"
        ));
        assert!(!protocol.exact_models.is_empty());
        assert!(repository_root().join(&protocol.evidence_source).exists());
        assert_eq!(protocol.offline_conformance, "Pass");
        assert_eq!(protocol.reviewed_at.len(), 10);
        assert_eq!(protocol.expires_at.len(), 10);
        assert!(!protocol.owner.is_empty());
        assert!(!protocol.removal_policy.is_empty());
        assert!(!protocol.known_limitations.is_empty());
        assert!(!protocol.capabilities.is_empty());
        for capability in &protocol.capabilities {
            assert!(!capability.case.is_empty());
            assert!(matches!(
                capability.status.as_str(),
                "Supported" | "Experimental" | "Unsupported" | "Unknown"
            ));
        }
        if protocol.provider_id == "official-openai" {
            assert_eq!(protocol.last_canary_attempt.len(), 10);
            assert_eq!(protocol.last_canary_success.len(), 10);
        } else {
            assert!(protocol.last_canary_success.is_empty());
        }
    }
}

fn validate_provider_entries(matrix: &Matrix) {
    let descriptors = support::conformance::conformance_cases();
    let mut keys = BTreeSet::new();
    for entry in &matrix.entries {
        let key = format!(
            "{}/{}/{}",
            entry.provider_id, entry.product_id, entry.exact_model
        );
        assert!(keys.insert(key.clone()), "duplicate matrix key {key}");
        let descriptor = descriptors
            .iter()
            .find(|candidate| {
                candidate.provider == entry.provider_id
                    && candidate.product == entry.product_id
                    && candidate.exact_model == entry.exact_model
            })
            .unwrap_or_else(|| panic!("missing conformance descriptor for {key}"));
        assert_eq!(entry.profile_version, descriptor.profile_version);
        assert_eq!(entry.workflow_id, descriptor.workflow_id);
        assert_eq!(entry.protocol_id, "openai-chat-completions");
        assert_eq!(entry.stability_level, "Experimental");
        assert_eq!(entry.owner, "Provider Compatibility");
        assert_eq!(entry.catalog_version, descriptor.catalog_version);
        assert_eq!(entry.compat_version, descriptor.compat_version);
        assert_eq!(entry.contract_version, "provider-fixture-v1");
        assert_eq!(entry.catalog_status, "Experimental");
        assert_eq!(entry.effective_status, "Experimental");
        assert_eq!(
            entry.evidence_levels,
            ["OfflineContractVerified", "RealProviderVerified"]
        );
        assert_eq!(entry.offline_evidence_id, "third-party-offline-contracts");
        assert_eq!(entry.fixture_manifest, descriptor.fixture_manifest);
        assert!(repository_root().join(&entry.evidence_source).is_file());
        assert_eq!(entry.reviewed_at, descriptor.reviewed_at);
        assert_eq!(entry.expires_at, descriptor.evidence_expires_at);
        assert_eq!(entry.last_canary_attempt.len(), 10);
        assert_eq!(entry.last_canary_success.len(), 10);
        assert!(!entry.removal_policy.is_empty());
        assert!(entry.limitations.iter().all(|item| !item.is_empty()));
        assert_eq!(entry.online_status, "Pass");
        let hosted = !entry.candidate_sha.is_empty() || !entry.run_url.is_empty();
        if hosted {
            assert_hosted_evidence(entry, &key);
        } else {
            assert!(entry.candidate_sha.is_empty(), "{key} empty SHA expected");
            assert!(entry.run_url.is_empty(), "{key} empty run_url expected");
        }

        let runtime = descriptor.profile.build("matrix-contract-credential");
        let model = philo::ModelId::new(entry.exact_model.clone()).unwrap();
        let catalog = runtime.model_entry(&model).expect("exact catalog entry");
        assert_eq!(catalog.support_status, CapabilityStatus::Supported);
        assert_eq!(catalog.source.reviewed_at(), entry.reviewed_at);
        assert_eq!(catalog.source.expires_at(), Some(entry.expires_at.as_str()));

        for cell in &entry.capabilities {
            let case = support::conformance::OnlineCase::ALL
                .into_iter()
                .find(|candidate| candidate.as_str() == cell.case)
                .unwrap_or_else(|| panic!("unknown matrix case {}", cell.case));
            let expected = format!("{:?}", descriptor.capabilities[&case]);
            assert_eq!(cell.status, expected, "capability drift for {key}");
            if cell.status == "Unknown" {
                assert_eq!(cell.evidence_id, "none");
            } else {
                assert!(
                    cell.evidence_id.starts_with("offline:")
                        || cell.evidence_id.starts_with("online:")
                        || cell.evidence_id.starts_with("hosted:"),
                    "capability evidence must be offline:/online:/hosted: for {key}"
                );
            }
        }
    }
}

#[test]
fn capability_decision_and_evidence_freshness_are_independent() {
    let runtime = DeepSeekProfile::from_api_key(CREDENTIAL_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let model = ModelId::new("deepseek-v4-flash").unwrap();
    let entry = runtime.model_entry(&model).unwrap();
    assert_eq!(entry.support_status, CapabilityStatus::Supported);
    assert!(!entry.source.is_stale_on("2026-10-23").unwrap());
    assert!(entry.source.is_stale_on("2026-10-24").unwrap());
    assert_eq!(entry.support_status, CapabilityStatus::Supported);

    let distinct = BTreeSet::from([
        format!("{:?}", CapabilityStatus::Supported),
        format!("{:?}", CapabilityStatus::Unsupported),
        format!("{:?}", CapabilityStatus::Unknown),
    ]);
    assert_eq!(distinct.len(), 3);
}

#[test]
fn structured_matrix_matches_catalog_conformance_and_evidence_policy() {
    let matrix = load_matrix();
    assert_eq!(matrix.schema_version, 2);
    assert_eq!(matrix.generated_as_of, "2026-07-29");
    assert_eq!(
        matrix.status_vocabulary,
        [
            "Supported",
            "Experimental",
            "Unsupported",
            "Unknown",
            "Stale"
        ]
    );
    assert_eq!(
        matrix.evidence_level_vocabulary,
        ["OfflineContractVerified", "RealProviderVerified"]
    );
    assert_eq!(matrix.entries.len(), 4);
    validate_official_protocols(&matrix);
    validate_provider_entries(&matrix);
}

#[test]
fn conformance_and_canary_allowlists_match_the_support_registry() {
    let matrix = load_matrix();
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/canary.yml")).unwrap();
    let descriptors = support::conformance::conformance_cases();
    for entry in matrix.entries {
        assert!(
            descriptors.iter().any(|descriptor| {
                descriptor.workflow_id == entry.workflow_id
                    && descriptor.provider == entry.provider_id
                    && descriptor.product == entry.product_id
                    && descriptor.exact_model == entry.exact_model
            }),
            "registry entry lacks matching conformance descriptor"
        );
        assert!(
            workflow.contains(&format!("- {}", entry.workflow_id)),
            "Canary workflow lacks registry workflow_id {}",
            entry.workflow_id
        );
    }
}

#[test]
fn body_axis_extension_diagnostic_stays_with_protocol_options() {
    use philo::protocol_options::{
        OpenAiChatOptions, OpenAiChatRawExtension, ProtocolOptionDiagnostic,
    };

    let raw = OpenAiChatRawExtension::dangerous_from_object(serde_json::json!({
        "provider": { "sort": "latency", "note": "diagnostics-canary-value" }
    }))
    .unwrap();
    let options = OpenAiChatOptions::new().with_raw_extension(raw);
    assert_eq!(
        options.diagnostics(),
        vec![ProtocolOptionDiagnostic::NonPortableExtensionUsed]
    );
    assert!(!format!("{options:?}").contains("diagnostics-canary-value"));
}
