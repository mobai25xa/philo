use serde::Serialize;

use super::case::ConformanceCase;

/// Closed case result state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Passed,
    Failed,
    Skipped,
}

/// Value-free result for one named contract section.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub status: CaseStatus,
    pub reason_code: Option<&'static str>,
}

/// Deterministic conformance report bound to an exact candidate SHA.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConformanceReport {
    pub schema_version: u32,
    pub provider: String,
    pub product: String,
    pub exact_model: String,
    pub profile_version: String,
    pub catalog_version: String,
    pub compat_version: String,
    pub candidate_sha: String,
    pub run_url_present: bool,
    pub results: Vec<CaseResult>,
}

impl ConformanceReport {
    pub fn new(
        descriptor: &ConformanceCase,
        candidate_sha: &str,
        run_url_present: bool,
        results: Vec<CaseResult>,
    ) -> Self {
        Self::new_for_model(
            descriptor,
            descriptor.exact_model,
            candidate_sha,
            run_url_present,
            results,
        )
    }

    pub fn new_for_model(
        descriptor: &ConformanceCase,
        exact_model: &str,
        candidate_sha: &str,
        run_url_present: bool,
        mut results: Vec<CaseResult>,
    ) -> Self {
        let candidate_sha = validate_candidate_sha(candidate_sha);
        results.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            schema_version: 1,
            provider: descriptor.provider.to_owned(),
            product: descriptor.product.to_owned(),
            exact_model: exact_model.to_owned(),
            profile_version: descriptor.profile_version.to_owned(),
            catalog_version: descriptor.catalog_version.to_owned(),
            compat_version: descriptor.compat_version.to_owned(),
            candidate_sha,
            run_url_present,
            results,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("value-free report must serialize")
    }
}

pub(super) fn validate_candidate_sha(candidate_sha: &str) -> String {
    assert!(
        candidate_sha.len() == 40 && candidate_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "candidate SHA must be exactly 40 hexadecimal characters"
    );
    candidate_sha.to_ascii_lowercase()
}
