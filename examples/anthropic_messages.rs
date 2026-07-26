//! Streams one official Anthropic Messages request using typed protocol options.

use std::error::Error;

use futures_util::StreamExt as _;
use philo::{
    AnthropicMessagesOptions, AnthropicThinkingDisplay, ConfigSource, ConfigValue,
    EnvironmentSecretResolver, GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef,
    ProviderConfigLayer, ProviderConfigSnapshot, ProviderId, ProviderRegistry, SecretReference,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let secret = ProviderConfigLayer::new(ConfigSource::environment_secret(
        "env/anthropic-key",
        "ANTHROPIC_API_KEY",
    )?)
    .with_credential(ConfigValue::set(SecretReference::environment_variable(
        "ANTHROPIC_API_KEY",
    )?));
    let config = ProviderConfigSnapshot::official_anthropic()?.merge_layers([secret])?;
    let provider = ProviderId::new("official-anthropic")?;
    let runtime = ProviderRegistry::with_official_anthropic()?.build(
        &provider,
        &config,
        &EnvironmentSecretResolver,
    )?;
    let client = LlmClient::with_reqwest(runtime)?;

    let model = std::env::var("ANTHROPIC_MODEL")?;
    let options = GenerationOptions::new()
        .with_max_output_tokens(512)
        .with_protocol_options(
            AnthropicMessagesOptions::new()
                .with_adaptive_thinking(AnthropicThinkingDisplay::Omitted),
        );
    let request = GenerateRequest::new(
        ModelRef::new("official-anthropic", model)?,
        vec![
            Message::system("Answer accurately and briefly."),
            Message::user("Give one practical use for a type-safe LLM SDK."),
        ],
    )
    .with_options(options);

    let mut stream = client.stream(request).await?;
    while let Some(event) = stream.next().await {
        if let philo::AssistantEvent::TextDelta { delta, .. } = event? {
            print!("{delta}");
        }
    }
    println!();
    Ok(())
}
