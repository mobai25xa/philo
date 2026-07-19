//! Explicitly enabled official `OpenAI` smoke test; skipped by default.

use std::time::Duration;

use futures_util::StreamExt as _;
use philo::{
    AssistantEvent, GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef,
    OfficialOpenAiProfile, PHASE_ONE_CONTRACT_ID, PHASE_ONE_CONTRACT_VERSION,
};

const ENABLED: &str = "OPENAI_SMOKE_ENABLED";
const API_KEY: &str = "OPENAI_API_KEY";
const MODEL: &str = "OPENAI_SMOKE_MODEL";

#[tokio::test]
async fn official_openai_text_stream_smoke() {
    if std::env::var(ENABLED).as_deref() != Ok("true") {
        println!("smoke_status=skipped reason=disabled");
        return;
    }

    let key = std::env::var(API_KEY).expect("OPENAI_API_KEY is required when smoke is enabled");
    let model = std::env::var(MODEL).expect("OPENAI_SMOKE_MODEL is required when smoke is enabled");
    assert!(
        !model.is_empty()
            && model
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character)),
        "OPENAI_SMOKE_MODEL must be a log-safe model identifier"
    );

    let runtime = OfficialOpenAiProfile::from_api_key(key)
        .expect("smoke credential configuration failed")
        .build()
        .expect("official OpenAI runtime configuration failed");
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.openai.com/v1/chat/completions"
    );
    let client = LlmClient::with_reqwest(runtime).expect("smoke transport configuration failed");
    let options = GenerationOptions::new()
        .with_max_output_tokens(16)
        .with_timeout(Duration::from_secs(45))
        .expect("static smoke timeout is valid");
    let request = GenerateRequest::new(
        ModelRef::new("official-openai", &model).expect("smoke model is valid"),
        vec![Message::user("Reply with one short plain-text word.")],
    )
    .with_options(options);

    let mut stream = client.stream(request).await.expect("smoke request failed");
    let mut saw_text = false;
    let mut saw_done = false;
    let mut usage_known = false;
    let mut provider_request_id_present = false;
    while let Some(item) = stream.next().await {
        match item.expect("smoke stream failed") {
            AssistantEvent::Start {
                provider_request_id,
                ..
            } => provider_request_id_present = provider_request_id.is_some(),
            AssistantEvent::TextDelta { delta, .. } => saw_text |= !delta.is_empty(),
            AssistantEvent::Usage(_) => usage_known = true,
            AssistantEvent::Done { .. } => saw_done = true,
            _ => {}
        }
    }

    assert!(saw_text, "smoke response contained no text delta");
    assert!(saw_done, "smoke response did not satisfy finish + DONE");
    assert!(
        provider_request_id_present,
        "smoke response did not include ProviderRequestId"
    );
    println!(
        "smoke_status=passed contract={PHASE_ONE_CONTRACT_ID}/{PHASE_ONE_CONTRACT_VERSION} model={model} usage_known={usage_known} provider_request_id_present=true"
    );
}
