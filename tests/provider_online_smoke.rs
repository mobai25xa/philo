//! Protected, ignored online entry using the shared conformance descriptor contract.

mod support;

use std::time::Duration;

use philo::protocol_options::{OpenAiChatOptions, OpenAiChatRawExtension};
use philo::{
    AssistantMessage, GenerateRequest, GenerationOptions, LlmClient, LlmError, Message, ModelRef,
};
use serde_json::json;
use support::conformance::{
    CaseResult, CaseStatus, ConformanceCase, OnlineCase, conformance_cases, plan_online_for_model,
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
    let (options, raw_routing_enabled) =
        online_options(&descriptor, &workflow_id, plan.timeout_seconds);
    let request = GenerateRequest::new(
        ModelRef::new(descriptor.provider, model).unwrap(),
        vec![Message::user("Reply with one short word.")],
    )
    .with_options(options);
    let message = complete_with_retry(&client, &descriptor, request).await;
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
        "provider_conformance_status=passed provider={} candidate_sha={} case=text_stream request_id_present={} generation_id_present={} raw_routing_accepted={} attribution_headers_accepted={} report={}",
        descriptor.provider,
        candidate_sha,
        request_id_present,
        generation_id_present,
        raw_routing_enabled,
        raw_routing_enabled,
        report.to_json()
    );
}

fn online_options(
    descriptor: &ConformanceCase,
    workflow_id: &str,
    timeout_seconds: u64,
) -> (GenerationOptions, bool) {
    let mut options = GenerationOptions::new()
        .with_max_output_tokens(16)
        .with_timeout(Duration::from_secs(timeout_seconds))
        .unwrap();
    let raw_routing_enabled = workflow_id == "openrouter";
    if raw_routing_enabled {
        let routing = OpenAiChatRawExtension::dangerous_from_object(json!({
            "provider": { "sort": "throughput" }
        }))
        .expect("reviewed OpenRouter routing extension must remain valid");
        options =
            options.with_protocol_options(OpenAiChatOptions::new().with_raw_extension(routing));
        assert!(
            descriptor
                .expected_headers
                .iter()
                .any(|(name, _)| *name == "http-referer")
        );
    }
    (options, raw_routing_enabled)
}

async fn complete_with_retry(
    client: &LlmClient,
    descriptor: &ConformanceCase,
    request: GenerateRequest,
) -> AssistantMessage {
    let mut attempt = 0;
    loop {
        match client.complete(request.clone()).await {
            Ok(message) => return message,
            Err(error)
                if descriptor.provider == "zai"
                    && transient_capacity_error(&error)
                    && attempt < 2 =>
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(10 * attempt)).await;
            }
            Err(error) => panic!(
                "online text case failed: {}",
                redacted_failure_category(&error)
            ),
        }
    }
}

fn transient_capacity_error(error: &LlmError) -> bool {
    matches!(error, LlmError::HttpStatus(error) if matches!(error.status(), 429 | 529))
}

fn redacted_failure_category(error: &LlmError) -> &'static str {
    match error {
        LlmError::HttpStatus(error) if error.status() == 429 => "http-rate-limited",
        LlmError::HttpStatus(error) if error.status() == 529 => "http-overloaded",
        LlmError::HttpStatus(_) => "http-status",
        LlmError::Protocol(_) => "protocol",
        LlmError::Transport(_) => "transport",
        LlmError::Timeout(_) => "timeout",
        LlmError::Cancelled => "cancelled",
        _ => "other",
    }
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
