//! Protected, ignored online smoke for reviewed public custom provider definitions.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::StreamExt as _;
use philo::{
    AnthropicUsageCompat, ApiKey, AssistantEvent, AuthScheme, CompatPatch, EndpointConfig,
    EnvironmentSecretResolver, FinishReason, FinishReasonCompat, GenerateRequest,
    GenerationOptions, LlmClient, LlmError, MaxOutputTokensWireFormat, Message, ModelRef,
    PolicySource, ProductId, ProviderCapabilities, ProviderConfigError, ProviderDefinition,
    ProviderDeploymentConfig, ProviderId, ProviderRuntime, RequestControl, SecretReference,
    SecretResolver, StaticProviderFactory, UsageCompat,
};

const CREDENTIAL_ENV: &str = "PHILO_PROVIDER_CREDENTIAL";
const OPENROUTER_TARGET: &str = "custom-openrouter-definition";
const OPENROUTER_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b:free";
const ZAI_ANTHROPIC_TARGET: &str = "custom-zai-anthropic-definition";
const ZAI_ANTHROPIC_MODEL: &str = "glm-4.7-flash";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

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
                .with_provider_compat(
                    CompatPatch::from_source(PolicySource::ProviderProfile)
                        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens)
                        .with_finish_reason(FinishReasonCompat::AllowOneIdenticalDuplicate)
                        .with_usage(UsageCompat::OpenAiDropInconsistentReasoning),
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
    GenerateRequest::new(
        ModelRef::new(target.provider_id(), model)
            .expect("reviewed model identifier must be valid"),
        vec![Message::user(prompt)],
    )
    .with_options(
        GenerationOptions::new()
            .with_max_output_tokens(max_tokens)
            .with_timeout(REQUEST_TIMEOUT)
            .expect("static request timeout must be valid"),
    )
}

async fn run_text_stream(
    client: &LlmClient,
    target: CustomTarget,
    model: &str,
) -> StreamObservation {
    let mut stream = client
        .stream(request(target, model, "Reply with one short word.", 32))
        .await
        .expect("custom provider text stream must start");
    let mut observation = StreamObservation::default();
    while let Some(item) = stream.next().await {
        match item.expect("custom provider text stream must decode") {
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
    let invalid = request(
        target,
        "philo-controlled-invalid-model",
        "This bounded request must fail before producing content.",
        1,
    );
    let error = client
        .complete(invalid)
        .await
        .expect_err("reviewed invalid model must produce a safe client error");
    assert!(
        matches!(error, LlmError::HttpStatus(ref error) if (400..500).contains(&error.status())),
        "controlled invalid model must produce an HTTP 4xx"
    );
}

async fn run_explicit_cancellation(client: &LlmClient, target: CustomTarget, model: &str) {
    let control = RequestControl::new();
    let cancellation = control.cancellation_token().clone();
    let mut stream = client
        .stream_with_control(
            request(
                target,
                model,
                "Begin a longer bounded response and continue until stopped.",
                128,
            ),
            control,
        )
        .await
        .expect("cancellation stream must start");
    let first = tokio::time::timeout(Duration::from_secs(30), stream.next())
        .await
        .expect("cancellation stream must emit promptly")
        .expect("cancellation stream must emit one event");
    first.expect("cancellation stream must decode before cancellation");
    cancellation.cancel();
    drop(stream);
    assert!(cancellation.is_cancelled());
}

async fn run_drop_cancellation(client: &LlmClient, target: CustomTarget, model: &str) {
    let control = RequestControl::new();
    let cancellation = control.cancellation_token().clone();
    let mut stream = client
        .stream_with_control(
            request(
                target,
                model,
                "Begin another longer bounded response and continue until stopped.",
                128,
            ),
            control,
        )
        .await
        .expect("drop stream must start");
    let first = tokio::time::timeout(Duration::from_secs(30), stream.next())
        .await
        .expect("drop stream must emit promptly")
        .expect("drop stream must emit one event");
    first.expect("drop stream must decode before drop");
    drop(stream);
    assert!(
        cancellation.is_cancelled(),
        "dropping an incomplete public stream must request cancellation"
    );
}

#[tokio::test]
#[ignore = "requires a protected provider-conformance Environment and explicit approval"]
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
    run_safe_4xx(&client, target).await;
    run_explicit_cancellation(&client, target, &model).await;
    run_drop_cancellation(&client, target, &model).await;

    println!(
        "custom_provider_smoke_status=passed target={} protocol={} candidate_sha={} model={} text_present={} usage_present={} finish_present={} provider_request_id_present={} generation_id_present={} safe_4xx=passed cancellation=passed drop=passed",
        target.workflow_id(),
        target.protocol_id(),
        candidate_sha,
        model,
        observation.saw(StreamSignal::Text),
        observation.saw(StreamSignal::Usage),
        observation.saw(StreamSignal::Finish),
        observation.saw(StreamSignal::ProviderRequestId),
        observation.saw(StreamSignal::GenerationId),
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
    let caller_endpoint_variable = ["PHILO_PROVIDER", "_ENDPOINT"].concat();
    assert!(!source.contains(&caller_endpoint_variable));
    for forbidden in [
        ["response", " body"].concat(),
        ["header", " value"].concat(),
    ] {
        assert!(!source.contains(&forbidden));
    }
}
