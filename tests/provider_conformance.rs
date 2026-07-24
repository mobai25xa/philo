//! Shared descriptor, offline runner, report, fixture, and skip-policy contracts.

mod support;

use std::collections::BTreeSet;

use support::conformance::{
    CapabilityDeclaration, CaseResult, CaseStatus, ConformanceReport, OfflineSection, OnlineCase,
    OnlineRequirement, RedactedFailure, conformance_cases, contains_forbidden_value, plan_online,
    run_offline,
};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn all_descriptors_run_the_same_offline_contract_sections() {
    let expected = OfflineSection::ALL
        .into_iter()
        .map(|section| section.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    for descriptor in conformance_cases() {
        let results = run_offline(&descriptor)
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", descriptor.id));
        assert_eq!(
            results
                .iter()
                .map(|result| result.name.clone())
                .collect::<BTreeSet<_>>(),
            expected,
            "{}",
            descriptor.id
        );
        assert!(
            results
                .iter()
                .all(|result| result.status == CaseStatus::Passed)
        );
    }
}

#[test]
fn unsupported_and_unknown_capabilities_skip_by_declared_policy() {
    for descriptor in conformance_cases() {
        let plan = plan_online(&descriptor, SHA, OnlineCase::ALL);
        for result in plan.preflight_results() {
            let case = OnlineCase::ALL
                .into_iter()
                .find(|case| case.as_str() == result.name)
                .unwrap();
            match descriptor.capabilities[&case] {
                CapabilityDeclaration::Supported | CapabilityDeclaration::Experimental => {
                    unreachable!("executable cases do not produce preflight results")
                }
                CapabilityDeclaration::Unsupported | CapabilityDeclaration::Unknown => {
                    assert!(matches!(
                        result.status,
                        CaseStatus::Skipped | CaseStatus::Failed
                    ));
                    assert!(result.reason_code.is_some());
                }
            }
        }
        for case in OnlineCase::ALL {
            if matches!(
                descriptor.capabilities[&case],
                CapabilityDeclaration::Supported | CapabilityDeclaration::Experimental
            ) {
                assert!(plan.selected.contains(&case));
            }
        }
        assert!(plan.max_output_tokens <= 128);
        assert!(plan.timeout_seconds <= 90);
    }
}

#[test]
#[should_panic(expected = "supported online cases cannot be configured as skipped")]
fn supported_online_cases_cannot_pass_as_skipped() {
    let mut descriptor = conformance_cases().remove(0);
    descriptor
        .capabilities
        .insert(OnlineCase::TextStream, CapabilityDeclaration::Supported);
    descriptor.online.insert(
        OnlineCase::TextStream,
        OnlineRequirement::Skipped("invalid test mutation"),
    );
    let _ = plan_online(&descriptor, SHA, [OnlineCase::TextStream]);
}

#[test]
fn fixture_manifest_and_provenance_are_complete() {
    for descriptor in conformance_cases() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(descriptor.fixture_manifest);
        let text = std::fs::read_to_string(path).unwrap();
        let manifest: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(manifest["provider"].as_str(), Some(descriptor.provider));
        assert_eq!(manifest["product"].as_str(), Some(descriptor.product));
        assert!(manifest["reviewed_at"].as_str().is_some());
        assert!(manifest["evidence_expires_at"].as_str().is_some());
        assert_eq!(
            manifest["synthetic_conformance_claim"].as_bool(),
            Some(false)
        );
    }
}

#[test]
fn reports_are_deterministic_value_free_and_bound_to_exact_sha() {
    const SECRET: &str = "report-secret-canary";
    let descriptor = conformance_cases().remove(0);
    let results = vec![CaseResult {
        name: "text_stream".to_owned(),
        status: CaseStatus::Passed,
        reason_code: None,
    }];
    let first = ConformanceReport::new(&descriptor, SHA, true, results.clone()).to_json();
    let second = ConformanceReport::new(&descriptor, SHA, true, results).to_json();
    assert_eq!(first, second);
    assert!(first.contains(SHA));
    assert!(!contains_forbidden_value(
        &first,
        &[SECRET, "Bearer ", "sk-"]
    ));
}

#[test]
fn secret_canaries_never_reach_reports_or_failure_observations() {
    const SECRET: &str = "provider-secret-canary";
    let observation = RedactedFailure::observe(
        "authentication",
        Some(401),
        Some("invalid_api_key"),
        SECRET.as_bytes(),
    );
    let encoded = serde_json::to_string(&observation).unwrap();
    assert!(!encoded.contains(SECRET));
    assert_eq!(observation.body_length, SECRET.len());
    assert!(observation.body_digest.starts_with("fnv1a64:"));
}

#[tokio::test]
async fn all_profile_cases_prove_success_and_test_only_skip_paths() {
    let descriptors = conformance_cases();
    assert_eq!(descriptors.len(), 6);
    for descriptor in descriptors {
        assert!(run_offline(&descriptor).await.is_ok(), "{}", descriptor.id);
    }

    let test_only = conformance_cases().remove(5);
    let online = plan_online(&test_only, SHA, OnlineCase::ALL);
    assert!(online.selected.is_empty());
    assert!(
        online
            .preflight_results()
            .iter()
            .all(|result| result.status == CaseStatus::Skipped)
    );
    let report = online.into_report(&test_only, false, Vec::new());
    assert!(
        report
            .results
            .iter()
            .all(|result| result.status == CaseStatus::Skipped)
    );
}

#[test]
#[should_panic(expected = "every executable online case must have exactly one result")]
fn planned_cases_cannot_be_reported_as_passed_without_execution() {
    let descriptor = conformance_cases().remove(0);
    let plan = plan_online(&descriptor, SHA, [OnlineCase::TextStream]);
    let _ = plan.into_report(&descriptor, false, Vec::new());
}
