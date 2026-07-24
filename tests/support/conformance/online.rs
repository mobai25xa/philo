use std::collections::BTreeSet;

use super::case::{CapabilityDeclaration, ConformanceCase, OnlineCase, OnlineRequirement};
use super::report::{CaseResult, CaseStatus, ConformanceReport, validate_candidate_sha};

/// Bounded online execution plan produced without reading credentials.
#[derive(Clone, Debug)]
pub struct OnlinePlan {
    pub selected: Vec<OnlineCase>,
    pub max_output_tokens: u32,
    pub timeout_seconds: u64,
    descriptor_id: &'static str,
    exact_model: String,
    candidate_sha: String,
    preflight_results: Vec<CaseResult>,
}

impl OnlinePlan {
    /// Returns only cases decided before network execution (Unknown/Unsupported failures/skips).
    pub fn preflight_results(&self) -> &[CaseResult] {
        &self.preflight_results
    }

    /// Finalizes a report only after every executable case has produced a real result.
    pub fn into_report(
        self,
        descriptor: &ConformanceCase,
        run_url_present: bool,
        executed_results: Vec<CaseResult>,
    ) -> ConformanceReport {
        assert_eq!(
            descriptor.id, self.descriptor_id,
            "online plan and descriptor must have the same identity"
        );
        let expected = self
            .selected
            .iter()
            .map(|case| case.as_str())
            .collect::<BTreeSet<_>>();
        let actual = executed_results
            .iter()
            .map(|result| result.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual, expected,
            "every executable online case must have exactly one result"
        );
        assert_eq!(
            executed_results.len(),
            expected.len(),
            "online results must not contain duplicate case names"
        );
        assert!(
            executed_results
                .iter()
                .all(|result| result.status != CaseStatus::Skipped),
            "supported executable cases cannot be reported as skipped"
        );

        let mut results = self.preflight_results;
        results.extend(executed_results);
        ConformanceReport::new_for_model(
            descriptor,
            &self.exact_model,
            &self.candidate_sha,
            run_url_present,
            results,
        )
    }
}

pub fn plan_online(
    descriptor: &ConformanceCase,
    candidate_sha: &str,
    selected: impl IntoIterator<Item = OnlineCase>,
) -> OnlinePlan {
    plan_online_for_model(descriptor, descriptor.exact_model, candidate_sha, selected)
}

pub fn plan_online_for_model(
    descriptor: &ConformanceCase,
    exact_model: &str,
    candidate_sha: &str,
    selected: impl IntoIterator<Item = OnlineCase>,
) -> OnlinePlan {
    assert!(
        !exact_model.is_empty() && exact_model.len() <= 128,
        "exact model must be non-empty and bounded"
    );
    let candidate_sha = validate_candidate_sha(candidate_sha);
    let selected = selected.into_iter().collect::<BTreeSet<_>>();
    let mut executable = Vec::new();
    let mut results = Vec::new();
    for case in OnlineCase::ALL {
        if !selected.contains(&case) {
            continue;
        }
        let capability = descriptor.capabilities[&case];
        let requirement = descriptor.online[&case];
        let (status, reason) = match (capability, requirement) {
            (
                CapabilityDeclaration::Supported | CapabilityDeclaration::Experimental,
                OnlineRequirement::Skipped(_),
            ) => {
                panic!("supported online cases cannot be configured as skipped")
            }
            (CapabilityDeclaration::Supported | CapabilityDeclaration::Experimental, _) => {
                executable.push(case);
                continue;
            }
            (CapabilityDeclaration::Unsupported, OnlineRequirement::Required) => {
                (CaseStatus::Failed, Some("required_but_unsupported"))
            }
            (CapabilityDeclaration::Unsupported, _) => {
                (CaseStatus::Skipped, Some("capability_unsupported"))
            }
            (CapabilityDeclaration::Unknown, OnlineRequirement::Required) => {
                (CaseStatus::Failed, Some("required_but_unknown"))
            }
            (CapabilityDeclaration::Unknown, _) => {
                (CaseStatus::Skipped, Some("capability_unknown"))
            }
        };
        results.push(CaseResult {
            name: case.as_str().to_owned(),
            status,
            reason_code: reason,
        });
    }
    OnlinePlan {
        selected: executable,
        max_output_tokens: 128,
        timeout_seconds: 90,
        descriptor_id: descriptor.id,
        exact_model: exact_model.to_owned(),
        candidate_sha,
        preflight_results: results,
    }
}
