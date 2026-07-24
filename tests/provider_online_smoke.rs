//! Protected, ignored online entry using the shared conformance descriptor contract.

mod support;

use std::time::Duration;

use philo::{GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef};
use support::conformance::{
    CaseResult, CaseStatus, OnlineCase, conformance_cases, plan_online_for_model,
};

#[tokio::test]
#[ignore = "requires protected provider-conformance environment and explicit opt-in"]
async fn protected_provider_conformance_smoke() {
    if std::env::var("PHILO_PROVIDER_ONLINE_ENABLED").as_deref() != Ok("true") {
        return;
    }
    let workflow_id = safe_identifier("PHILO_PROVIDER");
    let model = safe_model("PHILO_PROVIDER_MODEL");
    let candidate_sha = safe_sha("PHILO_PROVIDER_CANDIDATE_SHA");
    let key = std::env::var("PHILO_PROVIDER_CREDENTIAL")
        .expect("selected protected credential is required");

    let descriptor = conformance_cases()
        .into_iter()
        .find(|case| case.workflow_id == workflow_id)
        .expect("provider input must be allowlisted by a descriptor");
    let plan = plan_online_for_model(
        &descriptor,
        &model,
        &candidate_sha,
        [OnlineCase::TextStream, OnlineCase::UsageAndRequestId],
    );
    assert!(plan.selected.contains(&OnlineCase::TextStream));

    let runtime = descriptor.profile.build(&key);
    let client = LlmClient::with_reqwest(runtime).expect("HTTPS transport must build");
    let request = GenerateRequest::new(
        ModelRef::new(descriptor.provider, model).unwrap(),
        vec![Message::user("Reply with one short word.")],
    )
    .with_options(
        GenerationOptions::new()
            .with_max_output_tokens(16)
            .with_timeout(Duration::from_secs(plan.timeout_seconds))
            .unwrap(),
    );
    let message = client
        .complete(request)
        .await
        .expect("online text case failed");
    if descriptor.request_id_expected {
        assert!(message.provider_request_id().is_some());
    }
    if descriptor.usage_expected {
        assert!(message.usage().is_some());
    }
    let report = plan.into_report(
        &descriptor,
        std::env::var_os("GITHUB_RUN_ID").is_some(),
        vec![
            CaseResult {
                name: OnlineCase::TextStream.as_str().to_owned(),
                status: CaseStatus::Passed,
                reason_code: None,
            },
            CaseResult {
                name: OnlineCase::UsageAndRequestId.as_str().to_owned(),
                status: CaseStatus::Passed,
                reason_code: None,
            },
        ],
    );
    println!(
        "provider_conformance_status=passed provider={} candidate_sha={} case=text_stream request_id_present=true report={}",
        descriptor.provider,
        candidate_sha,
        report.to_json()
    );
}

fn safe_model(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/')
            }),
        "{name} must be a bounded exact model identifier"
    );
    value
}

fn safe_identifier(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "{name} must be an allowlisted identifier"
    );
    value
}

fn safe_sha(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{name} must be an exact 40-character SHA"
    );
    value.to_ascii_lowercase()
}
