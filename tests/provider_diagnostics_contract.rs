//! P3-014 value-free diagnostics and structured support-matrix contracts.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use http::{HeaderName, HeaderValue};
use philo::{
    AuthSchemeKind, CredentialSourceKind, DeepSeekProfile, EffectiveSupportStatus,
    EvidenceVerification, GenerateRequest, GenerationOptions, HeaderSource, Message, ModelRef,
    OpenRouterAttribution, OpenRouterProfile, OpenRouterRoutingPatch, PolicySource,
    ProviderRequestOptions, RequestMetadata, RoutingSort, SupportStatus,
};
use serde::Deserialize;

const CREDENTIAL_CANARY: &str = "diagnostics-credential-canary";
const PROMPT_CANARY: &str = "diagnostics-prompt-canary";
const HEADER_CANARY: &str = "diagnostics-header-value-canary";
const METADATA_CANARY: &str = "diagnostics-metadata-value-canary";
const ATTRIBUTION_CANARY: &str = "diagnostics-attribution-value-canary";

#[derive(Debug, Deserialize)]
struct Matrix {
    schema_version: u32,
    generated_as_of: String,
    status_vocabulary: Vec<String>,
    evidence_level_vocabulary: Vec<String>,
    entries: Vec<MatrixEntry>,
}

#[derive(Debug, Deserialize)]
struct MatrixEntry {
    provider_id: String,
    product_id: String,
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
    candidate_sha: String,
    run_url: String,
    online_status: String,
    reviewed_at: String,
    expires_at: String,
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

fn matrix_markdown_path() -> PathBuf {
    repository_root().join("support/provider-support-matrix.md")
}

fn load_matrix() -> Matrix {
    let text = fs::read_to_string(matrix_path()).expect("read support matrix");
    toml::from_str(&text).expect("parse support matrix")
}

fn openrouter_request() -> GenerateRequest {
    let mut metadata = RequestMetadata::new();
    metadata.insert("diagnostic-test", METADATA_CANARY).unwrap();
    let options = GenerationOptions::new()
        .with_header(
            HeaderName::from_static("x-diagnostic-test"),
            HeaderValue::from_static(HEADER_CANARY),
        )
        .with_metadata(metadata);
    GenerateRequest::new(
        ModelRef::new("openrouter", "nvidia/nemotron-3-ultra-550b-a55b:free").unwrap(),
        vec![Message::user(PROMPT_CANARY)],
    )
    .with_options(options)
}

#[test]
fn diagnostics_explain_final_sources_without_values() {
    let runtime = OpenRouterProfile::from_api_key(CREDENTIAL_CANARY)
        .unwrap()
        .with_attribution(
            OpenRouterAttribution::new("https://diagnostics.example", ATTRIBUTION_CANARY).unwrap(),
        )
        .with_routing(OpenRouterRoutingPatch::from_source(
            PolicySource::ProviderProfile,
        ))
        .build()
        .unwrap();
    let options = ProviderRequestOptions::new().with_openrouter_routing(
        OpenRouterRoutingPatch::from_source(PolicySource::Request).with_sort(RoutingSort::Latency),
    );
    let diagnostics = runtime
        .diagnostics_for_request(&openrouter_request(), &options, "2026-07-24")
        .unwrap();

    assert_eq!(diagnostics.provider_id().as_str(), "openrouter");
    assert_eq!(diagnostics.product_id().as_str(), "openrouter-chat");
    assert_eq!(
        diagnostics.domain_model().as_str(),
        "nvidia/nemotron-3-ultra-550b-a55b:free"
    );
    assert_eq!(
        diagnostics.provider_model().as_str(),
        "nvidia/nemotron-3-ultra-550b-a55b:free"
    );
    assert_eq!(
        diagnostics.wire_model().as_str(),
        "nvidia/nemotron-3-ultra-550b-a55b:free"
    );
    assert_eq!(diagnostics.endpoint().origin().host(), "openrouter.ai");
    assert_eq!(
        diagnostics.endpoint().path_shape(),
        "/api/v1/chat/completions"
    );
    assert_eq!(diagnostics.auth().scheme(), AuthSchemeKind::Bearer);
    assert_eq!(
        diagnostics.auth().credential_source(),
        CredentialSourceKind::Static
    );
    assert_eq!(
        diagnostics.support().status(),
        EffectiveSupportStatus::Experimental
    );
    assert_eq!(
        diagnostics.support().verification(),
        EvidenceVerification::CatalogDeclaration
    );
    assert_eq!(diagnostics.typed_extensions(), &["openrouter-routing"]);
    assert_eq!(diagnostics.compat().len(), 16);
    assert!(diagnostics.compat().iter().any(|entry| {
        entry.field() == philo::CompatField::ResponseFinishReason
            && entry.value() == "AllowOneIdenticalDuplicate"
            && entry.source() == PolicySource::ProviderProfile
    }));
    assert!(
        diagnostics
            .compat()
            .iter()
            .all(|entry| !entry.value().is_empty())
    );
    assert!(diagnostics.headers().iter().any(|entry| {
        entry.name().as_str() == "authorization"
            && entry.source() == HeaderSource::Auth
            && entry.is_protected()
            && entry.is_sensitive()
    }));
    assert!(diagnostics.headers().iter().any(|entry| {
        entry.name().as_str() == "http-referer"
            && entry.source() == HeaderSource::Provider
            && entry.is_protected()
            && !entry.is_sensitive()
    }));
    assert!(diagnostics.headers().iter().any(|entry| {
        entry.name().as_str() == "x-diagnostic-test"
            && entry.source() == HeaderSource::Request
            && !entry.is_sensitive()
    }));

    let formatted = format!("{diagnostics:?}\n{diagnostics}");
    for forbidden in [
        CREDENTIAL_CANARY,
        PROMPT_CANARY,
        HEADER_CANARY,
        METADATA_CANARY,
        ATTRIBUTION_CANARY,
        "https://diagnostics.example",
    ] {
        assert!(
            !formatted.contains(forbidden),
            "diagnostics leaked {forbidden}"
        );
    }
}

#[test]
fn exact_compat_and_expiry_are_visible_without_silent_upgrade() {
    let runtime = DeepSeekProfile::from_api_key(CREDENTIAL_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let request = GenerateRequest::new(
        ModelRef::new("deepseek", "deepseek-v4-flash").unwrap(),
        vec![Message::user("safe")],
    );
    let current = runtime
        .diagnostics_for_request(&request, &ProviderRequestOptions::new(), "2026-10-23")
        .unwrap();
    assert!(current.compat().iter().any(
        |entry| entry.value() == "MaxTokens" && entry.source() == PolicySource::ProviderProfile
    ));
    assert_eq!(
        current.support().status(),
        EffectiveSupportStatus::Experimental
    );

    let stale = runtime
        .diagnostics_for_request(&request, &ProviderRequestOptions::new(), "2026-10-24")
        .unwrap();
    assert_eq!(stale.support().status(), EffectiveSupportStatus::Stale);
    assert_eq!(stale.support().expires_at(), Some("2026-10-23"));

    let distinct = BTreeSet::from([
        format!("{:?}", EffectiveSupportStatus::Supported),
        format!("{:?}", EffectiveSupportStatus::Experimental),
        format!("{:?}", EffectiveSupportStatus::Unsupported),
        format!("{:?}", EffectiveSupportStatus::Unknown),
        format!("{:?}", EffectiveSupportStatus::Stale),
    ]);
    assert_eq!(distinct.len(), 5);
}

#[test]
fn structured_matrix_matches_catalog_conformance_and_evidence_policy() {
    let matrix = load_matrix();
    assert_eq!(matrix.schema_version, 1);
    assert_eq!(matrix.generated_as_of, "2026-07-24");
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
        assert_eq!(entry.catalog_version, descriptor.catalog_version);
        assert_eq!(entry.compat_version, descriptor.compat_version);
        assert_eq!(entry.contract_version, "provider-fixture-v1");
        assert_eq!(entry.catalog_status, "Experimental");
        assert_eq!(entry.effective_status, "Experimental");
        assert_eq!(
            entry.evidence_levels,
            ["OfflineContractVerified", "RealProviderVerified"]
        );
        assert_eq!(entry.offline_evidence_id, "p3-012-real-providers");
        assert_eq!(entry.fixture_manifest, descriptor.fixture_manifest);
        assert_eq!(entry.reviewed_at, descriptor.reviewed_at);
        assert_eq!(entry.expires_at, descriptor.evidence_expires_at);
        assert!(entry.limitations.iter().all(|item| !item.is_empty()));
        assert_eq!(entry.online_status, "Pass");
        // Hosted entries bind a frozen 40-char candidate SHA and Actions run URL.
        // Non-hosted exact products may keep both empty while remaining Experimental.
        let hosted = !entry.candidate_sha.is_empty() || !entry.run_url.is_empty();
        if hosted {
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
                entry.limitations.iter().any(|item| item == "hosted-protected-online"),
                "{key} must record hosted-protected-online limitation tag"
            );
        } else {
            assert!(entry.candidate_sha.is_empty(), "{key} empty SHA expected");
            assert!(entry.run_url.is_empty(), "{key} empty run_url expected");
        }

        let runtime = descriptor.profile.build("matrix-contract-credential");
        let model = philo::ModelId::new(entry.exact_model.clone()).unwrap();
        let catalog = runtime.model_entry(&model).expect("exact catalog entry");
        assert_eq!(catalog.support_status, SupportStatus::Experimental);
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
fn docs_matrix_is_a_checked_rendering_of_the_structured_source() {
    let matrix = load_matrix();
    let markdown = fs::read_to_string(matrix_markdown_path()).expect("read matrix Markdown");
    assert!(markdown.contains("<!-- BEGIN GENERATED SUPPORT MATRIX -->"));
    assert!(markdown.contains("<!-- END GENERATED SUPPORT MATRIX -->"));
    for entry in matrix.entries {
        let row = format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            entry.provider_id,
            entry.product_id,
            entry.exact_model,
            entry.profile_version,
            entry.catalog_status,
            entry.effective_status,
            entry.evidence_levels.join(","),
            entry.online_status,
            entry.expires_at
        );
        assert!(markdown.contains(&row), "missing generated row: {row}");
    }
    assert!(Path::new(&repository_root().join("support/provider-limitations.md")).exists());
    for forbidden in [
        CREDENTIAL_CANARY,
        PROMPT_CANARY,
        HEADER_CANARY,
        METADATA_CANARY,
    ] {
        assert!(!markdown.contains(forbidden));
    }
}
