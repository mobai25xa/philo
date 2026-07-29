//! Protected, ignored online smoke for reviewed public custom provider definitions.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::StreamExt as _;
use philo::domain::history::PolicySource;
use philo::domain::ids::ToolName;
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::{ToolChoice, ToolDefinition};
use philo::error::ProviderConfigError;
use philo::protocol_options::{AnthropicMessagesOptions, AnthropicThinkingDisplay};
use philo::provider::auth::ApiKey;
use philo::provider::capability::{ModelCapabilityProfile, ProviderCapabilities};
use philo::provider::catalog::ProductId;
use philo::provider::definition::AuthScheme;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::factory::StaticProviderFactory;
use philo::provider::protocol_contract::{
    AnthropicUsageCompat, CompatProfile, FinishReasonCompat, MaxOutputTokensWireFormat, UsageCompat,
};
use philo::provider::secret::{EnvironmentSecretResolver, SecretReference, SecretResolver};
use philo::transport::CancellationToken;
use philo::{
    AssistantEvent, AssistantMessage, AssistantStream, ContentPart, FinishReason, GenerateRequest,
    GenerationOptions, LlmClient, LlmError, Message, ModelId, ModelRef, ProviderDefinition,
    ProviderDeploymentConfig, ProviderId, ProviderRuntime, RequestControl,
};
use serde_json::json;

const CREDENTIAL_ENV: &str = "PHILO_PROVIDER_CREDENTIAL";
const OPENROUTER_TARGET: &str = "custom-openrouter-definition";
const OPENROUTER_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b:free";
const ZAI_ANTHROPIC_TARGET: &str = "custom-zai-anthropic-definition";
const ZAI_ANTHROPIC_MODEL: &str = "glm-4.7-flash";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const ZAI_INTER_CASE_DELAY: Duration = Duration::from_secs(30);
const MAX_TRANSIENT_START_RETRIES: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CustomTarget {
    OpenRouter,
    ZaiAnthropic,
}

impl CustomTarget {
    fn select(target: &str, model: &str) -> Self {
        match (target, model) {
            (OPENROUTER_TARGET, OPENROUTER_MODEL) => Self::OpenRouter,
            (ZAI_ANTHROPIC_TARGET, ZAI_ANTHROPIC_MODEL) => Self::ZaiAnthropic,
            _ => panic!("custom provider target and exact model must match the reviewed allowlist"),
        }
    }

    const fn workflow_id(self) -> &'static str {
        match self {
            Self::OpenRouter => OPENROUTER_TARGET,
            Self::ZaiAnthropic => ZAI_ANTHROPIC_TARGET,
        }
    }

    const fn provider_id(self) -> &'static str {
        match self {
            Self::OpenRouter => "custom-openrouter",
            Self::ZaiAnthropic => "custom-zai-anthropic",
        }
    }

    const fn product_id(self) -> &'static str {
        match self {
            Self::OpenRouter => "chat-completions",
            Self::ZaiAnthropic => "messages",
        }
    }

    const fn protocol_id(self) -> &'static str {
        match self {
            Self::OpenRouter => "openai-chat-completions",
            Self::ZaiAnthropic => "anthropic-messages",
        }
    }

    const fn expected_endpoint(self) -> &'static str {
        match self {
            Self::OpenRouter => "https://openrouter.ai/api/v1/chat/completions",
            Self::ZaiAnthropic => "https://api.z.ai/api/anthropic/v1/messages",
        }
    }

    fn definition(self) -> Result<ProviderDefinition, LlmError> {
        let provider = ProviderId::new(self.provider_id())?;
        let product = ProductId::new(self.product_id())?;
        match self {
            Self::OpenRouter => ProviderDefinition::openai_chat(provider, product)
                .with_endpoint(EndpointConfig::base_and_path(
                    "https://openrouter.ai/api/v1",
                    "/chat/completions",
                )?)
                .bind_credential_to_endpoint_origin()
                .with_auth_scheme(AuthScheme::bearer())
                .with_capabilities(ProviderCapabilities::conservative_chat_completions())
                .allow_unregistered_models()
                .with_openai_chat_compat(
                    CompatProfile::openai_chat_default()
                        .with_max_output_tokens(
                            MaxOutputTokensWireFormat::MaxTokens,
                            PolicySource::ProviderProfile,
                        )
                        .with_finish_reason(
                            FinishReasonCompat::AllowOneIdenticalDuplicate,
                            PolicySource::ProviderProfile,
                        )
                        .with_usage(
                            UsageCompat::OpenAiDropInconsistentReasoning,
                            PolicySource::ProviderProfile,
                        ),
                )
                .build(),
            Self::ZaiAnthropic => ProviderDefinition::anthropic_messages(provider, product)
                .with_endpoint(EndpointConfig::base_and_path(
                    "https://api.z.ai/api/anthropic",
                    "/v1/messages",
                )?)
                .bind_credential_to_endpoint_origin()
                .with_auth_scheme(AuthScheme::bearer())
                .with_anthropic_usage_compat(AnthropicUsageCompat::AllowMonotonicStableFields)?
                .with_anthropic_version("2023-06-01")?
                .with_capabilities(ProviderCapabilities::conservative_messages())
                .with_model_capabilities(
                    ModelCapabilityProfile::new(ModelId::new(ZAI_ANTHROPIC_MODEL)?)
                        .with_function_tools(CapabilityStatus::Supported)
                        .with_tool_choice_required(CapabilityStatus::Supported)
                        .with_adaptive_thinking(CapabilityStatus::Supported),
                )
                .allow_unregistered_models()
                .build(),
        }
    }

    fn runtime(self, resolver: &dyn SecretResolver) -> Result<ProviderRuntime, LlmError> {
        let provider = ProviderId::new(self.provider_id())?;
        let credential = SecretReference::environment_variable(CREDENTIAL_ENV)?;
        let deployment = ProviderDeploymentConfig::new(provider, credential);
        StaticProviderFactory::new(self.definition()?).build_deployment(&deployment, resolver)
    }

    const fn inter_case_delay(self) -> Duration {
        match self {
            Self::OpenRouter => Duration::ZERO,
            Self::ZaiAnthropic => ZAI_INTER_CASE_DELAY,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StreamSignal {
    Text,
    Usage,
    ProviderRequestId,
    GenerationId,
    Finish,
}

#[derive(Default)]
struct StreamObservation(BTreeSet<StreamSignal>);

impl StreamObservation {
    fn record(&mut self, signal: StreamSignal) {
        self.0.insert(signal);
    }

    fn saw(&self, signal: StreamSignal) -> bool {
        self.0.contains(&signal)
    }
}

fn request(target: CustomTarget, model: &str, prompt: &str, max_tokens: u32) -> GenerateRequest {
    request_with_options(
        target,
        model,
        prompt,
        GenerationOptions::new().with_max_output_tokens(max_tokens),
    )
}

fn request_with_options(
    target: CustomTarget,
    model: &str,
    prompt: &str,
    options: GenerationOptions,
) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new(target.provider_id(), model)
            .expect("reviewed model identifier must be valid"),
        vec![Message::user(prompt)],
    )
    .with_options(
        options
            .with_timeout(REQUEST_TIMEOUT)
            .expect("static request timeout must be valid"),
    )
}

fn is_transient_capacity_error(error: &LlmError) -> bool {
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

async fn transient_backoff(target: CustomTarget, attempt: usize) {
    println!(
        "custom_provider_smoke_transient_retry=true target={} attempt={} delay_seconds={}",
        target.workflow_id(),
        attempt,
        target.inter_case_delay().as_secs(),
    );
    tokio::time::sleep(target.inter_case_delay()).await;
}

async fn start_stream_with_retry(
    client: &LlmClient,
    target: CustomTarget,
    model: &str,
    prompt: &str,
    max_tokens: u32,
    failure_label: &str,
) -> AssistantStream {
    for attempt in 0..=MAX_TRANSIENT_START_RETRIES {
        match client
            .stream(request(target, model, prompt, max_tokens))
            .await
        {
            Ok(stream) => return stream,
            Err(error)
                if target == CustomTarget::ZaiAnthropic
                    && is_transient_capacity_error(&error)
                    && attempt < MAX_TRANSIENT_START_RETRIES =>
            {
                transient_backoff(target, attempt + 1).await;
            }
            Err(error) => panic!("{failure_label}: {}", redacted_failure_category(&error)),
        }
    }
    unreachable!("bounded transient retry loop must return or panic")
}

async fn start_controlled_stream_with_retry(
    client: &LlmClient,
    target: CustomTarget,
    model: &str,
    prompt: &str,
    failure_label: &str,
) -> (AssistantStream, CancellationToken) {
    for attempt in 0..=MAX_TRANSIENT_START_RETRIES {
        let control = RequestControl::new();
        let cancellation = control.cancellation_token().clone();
        match client
            .stream_with_control(request(target, model, prompt, 128), control)
            .await
        {
            Ok(stream) => return (stream, cancellation),
            Err(error)
                if target == CustomTarget::ZaiAnthropic
                    && is_transient_capacity_error(&error)
                    && attempt < MAX_TRANSIENT_START_RETRIES =>
            {
                transient_backoff(target, attempt + 1).await;
            }
            Err(error) => panic!("{failure_label}: {}", redacted_failure_category(&error)),
        }
    }
    unreachable!("bounded transient retry loop must return or panic")
}

async fn pace_between_cases(target: CustomTarget) {
    if !target.inter_case_delay().is_zero() {
        tokio::time::sleep(target.inter_case_delay()).await;
    }
}

async fn complete_with_retry(
    client: &LlmClient,
    target: CustomTarget,
    request: GenerateRequest,
    failure_label: &str,
) -> AssistantMessage {
    for attempt in 0..=MAX_TRANSIENT_START_RETRIES {
        match client.complete(request.clone()).await {
            Ok(message) => return message,
            Err(error)
                if target == CustomTarget::ZaiAnthropic
                    && is_transient_capacity_error(&error)
                    && attempt < MAX_TRANSIENT_START_RETRIES =>
            {
                transient_backoff(target, attempt + 1).await;
            }
            Err(error) => panic!("{failure_label}: {}", redacted_failure_category(&error)),
        }
    }
    unreachable!("bounded transient retry loop must return or panic")
}

async fn run_anthropic_tool_call(client: &LlmClient, target: CustomTarget, model: &str) {
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": { "value": { "type": "string" } },
        "required": ["value"],
        "additionalProperties": false
    }))
    .expect("reviewed tool schema must be valid");
    let options = GenerationOptions::new()
        .with_max_output_tokens(128)
        .with_tools(vec![ToolDefinition::new(
            ToolName::new("echo_value").expect("reviewed tool name must be valid"),
            schema,
        )])
        .with_tool_choice(ToolChoice::Required);
    let message = complete_with_retry(
        client,
        target,
        request_with_options(
            target,
            model,
            "Call the declared tool with a short harmless value.",
            options,
        ),
        "custom provider tool call failed",
    )
    .await;
    assert_eq!(message.finish_reason(), &FinishReason::ToolCalls);
    assert!(
        message
            .content()
            .iter()
            .any(|part| matches!(part, ContentPart::ToolCall(_)))
    );
}

async fn run_anthropic_thinking_displays(client: &LlmClient, target: CustomTarget, model: &str) {
    for display in [
        AnthropicThinkingDisplay::Omitted,
        AnthropicThinkingDisplay::Summarized,
    ] {
        let options = GenerationOptions::new()
            .with_max_output_tokens(256)
            .with_protocol_options(AnthropicMessagesOptions::new().with_adaptive_thinking(display));
        let message = complete_with_retry(
            client,
            target,
            request_with_options(
                target,
                model,
                "Return one short answer after reasoning.",
                options,
            ),
            "custom provider thinking display failed",
        )
        .await;
        assert!(!message.text().is_empty());
        pace_between_cases(target).await;
    }
}

async fn run_text_stream(
    client: &LlmClient,
    target: CustomTarget,
    model: &str,
) -> StreamObservation {
    let mut stream = start_stream_with_retry(
        client,
        target,
        model,
        "Reply with one short word.",
        32,
        "custom provider text stream must start",
    )
    .await;
    let mut observation = StreamObservation::default();
    while let Some(item) = stream.next().await {
        let event = item.unwrap_or_else(|error| {
            panic!(
                "custom provider text stream must decode: {}",
                redacted_failure_category(&error)
            )
        });
        match event {
            AssistantEvent::Start {
                provider_request_id,
                generation_id,
                ..
            } => {
                if provider_request_id.is_some() {
                    observation.record(StreamSignal::ProviderRequestId);
                }
                if generation_id.is_some() {
                    observation.record(StreamSignal::GenerationId);
                }
            }
            AssistantEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    observation.record(StreamSignal::Text);
                }
            }
            AssistantEvent::Usage(_) | AssistantEvent::DetailedUsage(_) => {
                observation.record(StreamSignal::Usage);
            }
            AssistantEvent::Done { finish_reason } => {
                assert!(matches!(
                    finish_reason,
                    FinishReason::Stop | FinishReason::Length
                ));
                observation.record(StreamSignal::Finish);
            }
            _ => {}
        }
    }
    assert!(observation.saw(StreamSignal::Text));
    assert!(observation.saw(StreamSignal::Usage));
    assert!(observation.saw(StreamSignal::Finish));
    assert!(
        observation.saw(StreamSignal::ProviderRequestId)
            || observation.saw(StreamSignal::GenerationId),
        "custom provider must expose at least one value-free remote request identity presence bit"
    );
    observation
}

async fn run_safe_4xx(client: &LlmClient, target: CustomTarget) {
    for attempt in 0..=MAX_TRANSIENT_START_RETRIES {
        let invalid = request(
            target,
            "philo-controlled-invalid-model",
            "This bounded request must fail before producing content.",
            1,
        );
        match client.complete(invalid).await {
            Err(error)
                if target == CustomTarget::ZaiAnthropic
                    && is_transient_capacity_error(&error)
                    && attempt < MAX_TRANSIENT_START_RETRIES =>
            {
                transient_backoff(target, attempt + 1).await;
            }
            Err(LlmError::HttpStatus(error)) => {
                assert!(
                    (400..500).contains(&error.status()) && error.status() != 429,
                    "controlled invalid model must produce a non-transient HTTP 4xx"
                );
                return;
            }
            Err(error) => panic!(
                "controlled invalid model returned wrong category: {}",
                redacted_failure_category(&error)
            ),
            Ok(_) => panic!("controlled invalid model unexpectedly succeeded"),
        }
    }
    unreachable!("bounded transient retry loop must return or panic")
}

async fn run_explicit_cancellation(client: &LlmClient, target: CustomTarget, model: &str) {
    let (mut stream, cancellation) = start_controlled_stream_with_retry(
        client,
        target,
        model,
        "Begin a longer bounded response and continue until stopped.",
        "cancellation stream must start",
    )
    .await;
    let first = tokio::time::timeout(REQUEST_TIMEOUT, stream.next())
        .await
        .expect("cancellation stream must emit promptly")
        .expect("cancellation stream must emit one event");
    first.unwrap_or_else(|error| {
        panic!(
            "cancellation stream must decode before cancellation: {}",
            redacted_failure_category(&error)
        )
    });
    cancellation.cancel();
    drop(stream);
    assert!(cancellation.is_cancelled());
}

async fn run_drop_cancellation(client: &LlmClient, target: CustomTarget, model: &str) {
    let (mut stream, cancellation) = start_controlled_stream_with_retry(
        client,
        target,
        model,
        "Begin another longer bounded response and continue until stopped.",
        "drop stream must start",
    )
    .await;
    let first = tokio::time::timeout(REQUEST_TIMEOUT, stream.next())
        .await
        .expect("drop stream must emit promptly")
        .expect("drop stream must emit one event");
    first.unwrap_or_else(|error| {
        panic!(
            "drop stream must decode before drop: {}",
            redacted_failure_category(&error)
        )
    });
    drop(stream);
    assert!(
        cancellation.is_cancelled(),
        "dropping an incomplete public stream must request cancellation"
    );
}

#[tokio::test]
#[ignore = "requires a protected provider-canary environment and explicit approval"]
async fn protected_custom_provider_definition_smoke() {
    if std::env::var("PHILO_PROVIDER_ONLINE_ENABLED").as_deref() != Ok("true") {
        return;
    }
    let workflow_id = safe_identifier("PHILO_PROVIDER");
    let model = safe_model("PHILO_PROVIDER_MODEL");
    let candidate_sha = safe_sha("PHILO_PROVIDER_CANDIDATE_SHA");
    let target = CustomTarget::select(&workflow_id, &model);
    let runtime = target
        .runtime(&EnvironmentSecretResolver)
        .expect("reviewed custom provider definition must compile");
    assert_eq!(runtime.provider_id().as_str(), target.provider_id());
    assert_eq!(runtime.protocol_id().as_str(), target.protocol_id());
    assert_eq!(
        runtime.endpoint().url().as_str(),
        target.expected_endpoint()
    );
    let client = LlmClient::with_reqwest(runtime).expect("reviewed HTTPS transport must build");

    let observation = run_text_stream(&client, target, &model).await;
    if target == CustomTarget::ZaiAnthropic {
        pace_between_cases(target).await;
        run_anthropic_tool_call(&client, target, &model).await;
        pace_between_cases(target).await;
        run_anthropic_thinking_displays(&client, target, &model).await;
    }
    pace_between_cases(target).await;
    run_safe_4xx(&client, target).await;
    pace_between_cases(target).await;
    run_explicit_cancellation(&client, target, &model).await;
    pace_between_cases(target).await;
    run_drop_cancellation(&client, target, &model).await;

    println!(
        "custom_provider_smoke_status=passed target={} protocol={} candidate_sha={} model={} text_present={} usage_present={} finish_present={} provider_request_id_present={} generation_id_present={} tool_call={} thinking_omitted={} thinking_summarized={} safe_4xx=passed cancellation=passed drop=passed",
        target.workflow_id(),
        target.protocol_id(),
        candidate_sha,
        model,
        observation.saw(StreamSignal::Text),
        observation.saw(StreamSignal::Usage),
        observation.saw(StreamSignal::Finish),
        observation.saw(StreamSignal::ProviderRequestId),
        observation.saw(StreamSignal::GenerationId),
        target == CustomTarget::ZaiAnthropic,
        target == CustomTarget::ZaiAnthropic,
        target == CustomTarget::ZaiAnthropic,
    );
}

fn safe_model(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
            }),
        "{name} must be a bounded reviewed model identifier"
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

struct StaticResolver;

impl SecretResolver for StaticResolver {
    fn resolve(&self, _reference: &SecretReference) -> Result<ApiKey, ProviderConfigError> {
        Ok(ApiKey::new("offline-custom-provider-smoke-key").unwrap())
    }
}

#[test]
fn reviewed_custom_definitions_compile_with_exact_origins_and_protocols() {
    for target in [CustomTarget::OpenRouter, CustomTarget::ZaiAnthropic] {
        let runtime = target.runtime(&StaticResolver).unwrap();
        assert_eq!(runtime.provider_id().as_str(), target.provider_id());
        assert_eq!(runtime.protocol_id().as_str(), target.protocol_id());
        assert_eq!(
            runtime.endpoint().url().as_str(),
            target.expected_endpoint()
        );
        assert!(runtime.endpoint().url().query().is_none());
        assert!(runtime.endpoint().url().fragment().is_none());
    }
}

#[test]
fn target_and_model_pairs_are_exactly_allowlisted() {
    assert_eq!(
        CustomTarget::select(OPENROUTER_TARGET, OPENROUTER_MODEL),
        CustomTarget::OpenRouter
    );
    assert_eq!(
        CustomTarget::select(ZAI_ANTHROPIC_TARGET, ZAI_ANTHROPIC_MODEL),
        CustomTarget::ZaiAnthropic
    );
}

#[test]
fn smoke_source_keeps_endpoint_and_output_policy_explicit() {
    let source = include_str!("custom_provider_online_smoke.rs");
    assert!(source.contains("#[ignore ="));
    assert!(source.contains("bind_credential_to_endpoint_origin"));
    assert!(source.contains("https://openrouter.ai/api/v1"));
    assert!(source.contains("https://api.z.ai/api/anthropic"));
    assert!(source.contains("safe_4xx=passed cancellation=passed drop=passed"));
    assert!(source.contains("MAX_TRANSIENT_START_RETRIES"));
    assert!(source.contains("ZAI_INTER_CASE_DELAY"));
    assert!(source.contains("redacted_failure_category"));
    let forbidden_method = ["expect", "_err"].concat();
    assert!(!source.contains(&forbidden_method));
    let caller_endpoint_variable = ["PHILO_PROVIDER", "_ENDPOINT"].concat();
    assert!(!source.contains(&caller_endpoint_variable));
    for forbidden in [
        ["response", " body"].concat(),
        ["header", " value"].concat(),
    ] {
        assert!(!source.contains(&forbidden));
    }
}
