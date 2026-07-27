//! Explicitly enabled, sequential official `OpenAI` phase-two smoke suite.
#![allow(clippy::too_many_lines)]

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use futures_util::StreamExt as _;
use philo::domain::content::{ImageContent, ImageDetail};
use philo::domain::ids::ToolName;
use philo::domain::request::{
    CapabilityStatus, ReasoningEffort, ReasoningEffortSupport, ThinkingRequest,
};
use philo::domain::schema::ToolSchema;
use philo::domain::structured::{ResponseFormat, StructuredSchema};
use philo::domain::tools::{ParallelToolCalls, ToolChoice, ToolDefinition};
use philo::provider::capability::{ModelCapabilityProfile, OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE};
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::{
    AssistantEvent, ContentPart, FinishReason, GenerateRequest, GenerationOptions, LlmClient,
    Message, MessageRole, ModelId, ModelRef, PHASE_TWO_CONTRACT_ID, PHASE_TWO_CONTRACT_VERSION,
    TokenCount,
};
use serde_json::json;

const ENABLED: &str = "OPENAI_SMOKE_ENABLED";
const API_KEY: &str = "OPENAI_API_KEY";
const MODEL: &str = "OPENAI_SMOKE_MODEL";
const COMMIT: &str = "OPENAI_SMOKE_COMMIT";
const CAPABILITIES: &str = "OPENAI_SMOKE_CAPABILITIES";
const IMAGE_URL: &str = "OPENAI_SMOKE_IMAGE_URL";
const DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const ALLOWED_CAPABILITIES: [&str; 6] = [
    "tools",
    "parallel_tools",
    "json_object",
    "json_schema",
    "vision",
    "reasoning_high",
];

struct SmokeConfig {
    model: String,
    commit: String,
    capabilities: HashSet<String>,
    client: LlmClient,
}

impl SmokeConfig {
    fn from_environment() -> Self {
        let key = std::env::var(API_KEY).expect("OPENAI_API_KEY is required when smoke is enabled");
        let model = safe_identifier(MODEL);
        let commit = safe_identifier(COMMIT);
        let capabilities = parse_capabilities();
        let profile = exact_model_profile(&model, &capabilities);
        let runtime = OfficialOpenAiProfile::from_api_key(key)
            .expect("smoke credential configuration failed")
            .with_model_capabilities(profile)
            .build()
            .expect("official OpenAI runtime configuration failed");
        assert_eq!(
            runtime.endpoint().url().as_str(),
            "https://api.openai.com/v1/chat/completions"
        );
        let client =
            LlmClient::with_reqwest(runtime).expect("smoke transport configuration failed");
        Self {
            model,
            commit,
            capabilities,
            client,
        }
    }

    fn supports(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    fn request(&self, messages: Vec<Message>, options: GenerationOptions) -> GenerateRequest {
        GenerateRequest::new(
            ModelRef::new("official-openai", &self.model).expect("smoke model is valid"),
            messages,
        )
        .with_options(options)
    }

    fn log(&self, case: &str, summary: &str) {
        println!(
            "smoke_status=passed case={case} commit={} contract={}/{} profile=official-openai model={} review_date={} {summary}",
            self.commit,
            PHASE_TWO_CONTRACT_ID,
            PHASE_TWO_CONTRACT_VERSION,
            self.model,
            OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE,
        );
    }

    fn skip(&self, case: &str, capability: &str) {
        println!(
            "smoke_status=skipped case={case} capability={capability} reason=capability_unsupported commit={} contract={}/{} model={}",
            self.commit, PHASE_TWO_CONTRACT_ID, PHASE_TWO_CONTRACT_VERSION, self.model,
        );
    }
}

#[derive(Default)]
struct StreamSummary {
    starts: usize,
    text_starts: usize,
    text_deltas: usize,
    text_ends: usize,
    tool_starts: usize,
    tool_deltas: usize,
    tool_ends: usize,
    usage: usize,
    detailed_usage: usize,
    done: usize,
    provider_request_id_present: bool,
    finish_reason: Option<FinishReason>,
    wire_indexes: BTreeSet<u32>,
    tool_ids: BTreeSet<String>,
    tool_names: BTreeSet<String>,
}

async fn summarize_stream(
    client: &LlmClient,
    request: GenerateRequest,
) -> Result<StreamSummary, philo::LlmError> {
    let mut stream = client.stream(request).await?;
    let mut summary = StreamSummary::default();
    while let Some(item) = stream.next().await {
        match item? {
            AssistantEvent::Start {
                provider_request_id,
                ..
            } => {
                summary.starts += 1;
                summary.provider_request_id_present |= provider_request_id.is_some();
            }
            AssistantEvent::TextStart { .. } => summary.text_starts += 1,
            AssistantEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    summary.text_deltas += 1;
                }
            }
            AssistantEvent::TextEnd { .. } => summary.text_ends += 1,
            AssistantEvent::ToolCallStart { wire_index, .. } => {
                summary.tool_starts += 1;
                summary.wire_indexes.insert(wire_index.get());
            }
            AssistantEvent::ToolCallDelta { .. } => summary.tool_deltas += 1,
            AssistantEvent::ToolCallEnd { call, .. } => {
                summary.tool_ends += 1;
                summary.tool_ids.insert(call.id().as_str().to_owned());
                summary.tool_names.insert(call.name().as_str().to_owned());
                assert!(call.arguments().value().is_object());
            }
            AssistantEvent::Usage(_) => summary.usage += 1,
            AssistantEvent::DetailedUsage(_) => summary.detailed_usage += 1,
            AssistantEvent::Done { finish_reason } => {
                summary.done += 1;
                summary.finish_reason = Some(finish_reason);
            }
            _ => {}
        }
    }
    Ok(summary)
}

#[tokio::test]
async fn official_openai_phase_two_smoke_suite() {
    if std::env::var(ENABLED).as_deref() != Ok("true") {
        println!("smoke_status=skipped reason=disabled");
        return;
    }

    let config = SmokeConfig::from_environment();
    text_stream_smoke(&config).await;
    single_tool_smoke(&config).await;
    parallel_tool_smoke(&config).await;
    json_object_smoke(&config).await;
    json_schema_smoke(&config).await;
    vision_smoke(&config).await;
    reasoning_smoke(&config).await;
    println!(
        "smoke_suite_status=passed commit={} contract={}/{} profile=official-openai model={} result=pass",
        config.commit, PHASE_TWO_CONTRACT_ID, PHASE_TWO_CONTRACT_VERSION, config.model,
    );
}

async fn text_stream_smoke(config: &SmokeConfig) {
    let options = options(32);
    let request = config.request(
        vec![Message::user("Reply with one short plain-text word.")],
        options,
    );
    let summary = summarize_stream(&config.client, request)
        .await
        .expect("text smoke failed");
    assert_eq!(summary.starts, 1);
    assert_eq!(summary.text_starts, 1);
    assert!(summary.text_deltas > 0);
    assert_eq!(summary.text_ends, 1);
    assert!(summary.usage > 0);
    assert_eq!(summary.done, 1);
    assert!(summary.provider_request_id_present);
    assert!(matches!(summary.finish_reason, Some(FinishReason::Stop)));
    config.log(
        "text_stream",
        &format!(
            "provider_request_id_present=true events=start:{},text_start:{},text_delta:{},text_end:{},usage:{},done:{} finish=stop usage=known reasoning=unknown result=pass",
            summary.starts,
            summary.text_starts,
            summary.text_deltas,
            summary.text_ends,
            summary.usage,
            summary.done,
        ),
    );
}

async fn single_tool_smoke(config: &SmokeConfig) {
    if !config.supports("tools") {
        config.skip("single_tool", "tools");
        return;
    }
    let tool = city_tool("lookup_weather", "Return synthetic weather for a city");
    let request = config.request(
        vec![Message::user("Call lookup_weather exactly once for Paris.")],
        options(64)
            .with_tools(vec![tool])
            .with_tool_choice(ToolChoice::Specific {
                name: ToolName::new("lookup_weather").unwrap(),
            }),
    );
    let summary = summarize_stream(&config.client, request)
        .await
        .expect("single tool smoke failed");
    assert_eq!(summary.tool_starts, 1);
    assert!(summary.tool_deltas > 0);
    assert_eq!(summary.tool_ends, 1);
    assert_eq!(summary.tool_ids.len(), 1);
    assert!(summary.provider_request_id_present);
    assert_eq!(
        summary.tool_names,
        BTreeSet::from(["lookup_weather".to_owned()])
    );
    assert_eq!(summary.done, 1);
    assert!(matches!(
        summary.finish_reason,
        Some(FinishReason::ToolCalls)
    ));
    config.log(
        "single_tool",
        &format!(
            "provider_request_id_present={} events=tool_start:{},tool_delta:{},tool_end:{},usage:{},done:{} finish=tool_calls usage={} reasoning=unknown result=pass",
            summary.provider_request_id_present,
            summary.tool_starts,
            summary.tool_deltas,
            summary.tool_ends,
            summary.usage,
            summary.done,
            known(summary.usage > 0),
        ),
    );
}

async fn parallel_tool_smoke(config: &SmokeConfig) {
    if !config.supports("parallel_tools") {
        config.skip("parallel_tool", "parallel_tools");
        return;
    }
    let request = config.request(
        vec![Message::user(
            "Call both lookup_weather and lookup_time for Paris in this turn.",
        )],
        options(96)
            .with_tools(vec![
                city_tool("lookup_weather", "Return synthetic weather for a city"),
                city_tool("lookup_time", "Return synthetic local time for a city"),
            ])
            .with_tool_choice(ToolChoice::Required)
            .with_parallel_tool_calls(ParallelToolCalls::Enabled),
    );
    let summary = summarize_stream(&config.client, request)
        .await
        .expect("parallel tool smoke failed");
    assert_eq!(summary.wire_indexes.len(), 2);
    assert_eq!(summary.tool_ids.len(), 2);
    assert!(summary.provider_request_id_present);
    assert_eq!(summary.tool_ends, 2);
    assert_eq!(
        summary.tool_names,
        BTreeSet::from(["lookup_time".to_owned(), "lookup_weather".to_owned()])
    );
    assert_eq!(summary.done, 1);
    assert!(matches!(
        summary.finish_reason,
        Some(FinishReason::ToolCalls)
    ));
    config.log(
        "parallel_tool",
        &format!(
            "provider_request_id_present={} events=tool_start:{},tool_delta:{},tool_end:{},usage:{},done:{} wire_indexes=2 stable_ids=2 finish=tool_calls usage={} reasoning=unknown result=pass",
            summary.provider_request_id_present,
            summary.tool_starts,
            summary.tool_deltas,
            summary.tool_ends,
            summary.usage,
            summary.done,
            known(summary.usage > 0),
        ),
    );
}

async fn json_object_smoke(config: &SmokeConfig) {
    if !config.supports("json_object") {
        config.skip("json_object", "json_object");
        return;
    }
    let message = config
        .client
        .complete(config.request(
            vec![Message::user(
                "Return a JSON object with one key named answer and a short string value.",
            )],
            options(64).with_response_format(ResponseFormat::JsonObject),
        ))
        .await
        .expect("JSON object smoke failed");
    assert!(
        message
            .structured_output()
            .is_some_and(serde_json::Value::is_object)
    );
    assert!(matches!(message.finish_reason(), FinishReason::Stop));
    assert!(message.provider_request_id().is_some());
    config.log(
        "json_object",
        &format!(
            "provider_request_id_present={} events=complete:1 finish=stop usage={} reasoning=unknown root=object result=pass",
            message.provider_request_id().is_some(),
            known(message.usage().is_some()),
        ),
    );
}

async fn json_schema_smoke(config: &SmokeConfig) {
    if !config.supports("json_schema") {
        config.skip("json_schema", "json_schema");
        return;
    }
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": { "answer": { "type": "string", "minLength": 1 } },
        "required": ["answer"],
        "additionalProperties": false
    }))
    .unwrap();
    let structured = StructuredSchema::new("smoke_answer", None, schema, true).unwrap();
    let message = config
        .client
        .complete(config.request(
            vec![Message::user("Return the requested answer object for 2+2.")],
            options(64).with_response_format(ResponseFormat::JsonSchema(structured)),
        ))
        .await
        .expect("JSON schema smoke failed");
    assert!(
        message
            .structured_output()
            .is_some_and(|value| value["answer"].is_string())
    );
    assert!(matches!(message.finish_reason(), FinishReason::Stop));
    assert!(message.provider_request_id().is_some());
    config.log(
        "json_schema",
        &format!(
            "provider_request_id_present={} events=complete:1 finish=stop usage={} reasoning=unknown schema=pass result=pass",
            message.provider_request_id().is_some(),
            known(message.usage().is_some()),
        ),
    );
}

async fn vision_smoke(config: &SmokeConfig) {
    if !config.supports("vision") {
        config.skip("image_url", "vision");
        config.skip("image_data_url", "vision");
        return;
    }
    let image_url = std::env::var(IMAGE_URL)
        .expect("OPENAI_SMOKE_IMAGE_URL is required when vision is declared supported");
    let cases = [
        (
            "image_url",
            ImageContent::parse_url(&image_url, ImageDetail::Low)
                .expect("smoke image URL is invalid"),
        ),
        (
            "image_data_url",
            ImageContent::from_data_url(DATA_URL, ImageDetail::Low)
                .expect("embedded smoke data URL is invalid"),
        ),
    ];
    for (case, image) in cases {
        let message = config
            .client
            .complete(config.request(
                vec![Message::new(
                    MessageRole::User,
                    vec![
                        ContentPart::text("Describe the image in one word."),
                        ContentPart::Image(image),
                    ],
                )],
                options(64),
            ))
            .await
            .unwrap_or_else(|_| panic!("{case} smoke failed"));
        assert!(matches!(message.finish_reason(), FinishReason::Stop));
        assert!(message.provider_request_id().is_some());
        config.log(
            case,
            &format!(
                "provider_request_id_present={} events=complete:1 finish=stop usage={} reasoning=unknown image_payload_logged=false result=pass",
                message.provider_request_id().is_some(),
                known(message.usage().is_some()),
            ),
        );
    }
}

async fn reasoning_smoke(config: &SmokeConfig) {
    if !config.supports("reasoning_high") {
        config.skip("reasoning_high", "reasoning_high");
        return;
    }
    let message = config
        .client
        .complete(config.request(
            vec![Message::user("What is 17 multiplied by 19? Reply briefly.")],
            options(128).with_reasoning(ThinkingRequest::Effort(ReasoningEffort::High)),
        ))
        .await
        .expect("reasoning smoke failed");
    let reasoning = message
        .usage_details()
        .map_or(TokenCount::Unknown, philo::UsageDetails::reasoning_tokens);
    assert!(matches!(message.finish_reason(), FinishReason::Stop));
    assert!(message.provider_request_id().is_some());
    config.log(
        "reasoning_high",
        &format!(
            "provider_request_id_present={} events=complete:1 finish=stop usage={} reasoning={} result=pass",
            message.provider_request_id().is_some(),
            known(message.usage().is_some()),
            if reasoning.is_known() { "known" } else { "unknown" },
        ),
    );
}

fn options(max_output_tokens: u32) -> GenerationOptions {
    GenerationOptions::new()
        .with_max_output_tokens(max_output_tokens)
        .with_timeout(Duration::from_secs(90))
        .expect("static smoke timeout is valid")
}

fn city_tool(name: &str, description: &str) -> ToolDefinition {
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": { "city": { "type": "string", "minLength": 1 } },
        "required": ["city"],
        "additionalProperties": false
    }))
    .unwrap();
    ToolDefinition::new(ToolName::new(name).unwrap(), schema)
        .with_description(description)
        .unwrap()
}

fn exact_model_profile(model: &str, capabilities: &HashSet<String>) -> ModelCapabilityProfile {
    let supported = |name: &str| {
        if capabilities.contains(name) {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unknown
        }
    };
    let tools = capabilities.contains("tools") || capabilities.contains("parallel_tools");
    ModelCapabilityProfile::new(ModelId::new(model).unwrap())
        .with_function_tools(if tools {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unknown
        })
        .with_tool_choice_required(if tools {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unknown
        })
        .with_tool_choice_specific(if tools {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unknown
        })
        .with_parallel_tool_calls(supported("parallel_tools"))
        .with_vision_input(supported("vision"))
        .with_response_format_json_object(supported("json_object"))
        .with_response_format_json_schema(supported("json_schema"))
        .with_reasoning_efforts(if capabilities.contains("reasoning_high") {
            ReasoningEffortSupport::Supported(BTreeSet::from([ReasoningEffort::High]))
        } else {
            ReasoningEffortSupport::Unknown
        })
}

fn parse_capabilities() -> HashSet<String> {
    let raw = std::env::var(CAPABILITIES).unwrap_or_default();
    let capabilities = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    for capability in &capabilities {
        assert!(
            ALLOWED_CAPABILITIES.contains(&capability.as_str()),
            "unknown OPENAI_SMOKE_CAPABILITIES entry"
        );
    }
    capabilities
}

fn safe_identifier(name: &str) -> String {
    let value =
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is required when smoke is enabled"));
    assert!(
        !value.is_empty()
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character)),
        "{name} must be a log-safe identifier"
    );
    value
}

fn known(value: bool) -> &'static str {
    if value { "known" } else { "unknown" }
}
