#![allow(dead_code)]

use std::collections::BTreeSet;
use std::error::Error;

use philo::domain::request::{CapabilityStatus, ReasoningEffort, ReasoningEffortSupport};
use philo::provider::capability::ModelCapabilityProfile;
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::{GenerateRequest, LlmClient, Message, ModelId, ModelRef};

pub(crate) type ExampleResult<T = ()> = Result<T, Box<dyn Error>>;

pub(crate) fn client() -> ExampleResult<LlmClient> {
    let runtime = official_runtime_from_env()?;
    Ok(LlmClient::with_reqwest(runtime)?)
}

pub(crate) fn official_runtime_from_env() -> ExampleResult<philo::ProviderRuntime> {
    let key = std::env::var("OPENAI_API_KEY")?;
    Ok(OfficialOpenAiProfile::from_api_key(key)?.build()?)
}

pub(crate) fn model_from_env(provider: &str) -> ExampleResult<ModelRef> {
    Ok(ModelRef::new(provider, std::env::var("OPENAI_MODEL")?)?)
}

/// Builds a client whose exact model profile enables selected phase-two features.
pub(crate) fn client_with_phase2_capabilities() -> ExampleResult<LlmClient> {
    let key = std::env::var("OPENAI_API_KEY")?;
    let model = std::env::var("OPENAI_MODEL")?;
    let profile = ModelCapabilityProfile::new(ModelId::new(model)?)
        .with_function_tools(CapabilityStatus::Supported)
        .with_tool_choice_required(CapabilityStatus::Supported)
        .with_tool_choice_specific(CapabilityStatus::Supported)
        .with_parallel_tool_calls(CapabilityStatus::Supported)
        .with_strict_tools(CapabilityStatus::Supported)
        .with_vision_input(CapabilityStatus::Supported)
        .with_image_detail_original(CapabilityStatus::Supported)
        .with_response_format_json_object(CapabilityStatus::Supported)
        .with_response_format_json_schema(CapabilityStatus::Supported)
        .with_reasoning_efforts(ReasoningEffortSupport::Supported(BTreeSet::from([
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ])));
    let runtime = OfficialOpenAiProfile::from_api_key(key)?
        .with_model_capabilities(profile)
        .build()?;
    Ok(LlmClient::with_reqwest(runtime)?)
}

pub(crate) fn request(prompt: &str) -> ExampleResult<GenerateRequest> {
    let model = std::env::var("OPENAI_MODEL")?;
    Ok(GenerateRequest::new(
        ModelRef::new("official-openai", model)?,
        vec![Message::user(prompt)],
    ))
}

pub(crate) fn has_live_credentials() -> bool {
    std::env::var_os("OPENAI_API_KEY").is_some() && std::env::var_os("OPENAI_MODEL").is_some()
}
