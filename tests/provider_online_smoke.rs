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
        assert!(
            message.provider_request_id().is_some(),
            "provider_request_id expected for provider={} but was None",
            descriptor.provider
        );
    }
    if descriptor.generation_id_expected {
        assert!(
            message.generation_id().is_some(),
            "generation_id expected for provider={} but was None",
            descriptor.provider
        );
    }
    if descriptor.usage_expected {
        assert!(message.usage().is_some());
    }
    let request_id_present = message.provider_request_id().is_some();
    let generation_id_present = message.generation_id().is_some();
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
        "provider_conformance_status=passed provider={} candidate_sha={} case=text_stream request_id_present={} generation_id_present={} report={}",
        descriptor.provider,
        candidate_sha,
        request_id_present,
        generation_id_present,
        report.to_json()
    );
}

fn safe_model(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        valid_model_identifier(&value),
        "{name} must be a bounded exact model identifier"
    );
    value
}

fn valid_model_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 {
        return false;
    }
    let mut parts = value.split(':');
    let Some(base) = parts.next() else {
        return false;
    };
    let suffix = parts.next();
    if parts.next().is_some() || base.is_empty() || suffix.is_some_and(str::is_empty) {
        return false;
    }
    base.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        && suffix.is_none_or(|suffix| {
            suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
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

#[test]
fn exact_model_identifier_accepts_openrouter_variant_and_zai_model() {
    assert!(valid_model_identifier(
        "nvidia/nemotron-3-ultra-550b-a55b:free"
    ));
    assert!(valid_model_identifier("glm-4.7-flash"));
    assert!(!valid_model_identifier("model:"));
    assert!(!valid_model_identifier("model:free:extra"));
}
