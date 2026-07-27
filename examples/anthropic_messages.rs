//! Streams one official Anthropic Messages request using typed protocol options.

use std::error::Error;

use futures_util::StreamExt as _;
use philo::protocol_options::{AnthropicMessagesOptions, AnthropicThinkingDisplay};
use philo::provider::registry::ProviderRegistry;
use philo::provider::secret::{EnvironmentSecretResolver, SecretReference};
use philo::{
    GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef, ProviderDeploymentConfig,
    ProviderId,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    // One construction path: a registered definition plus a deployment that
    // names the credential. Layered configuration documents, if you want them,
    // live in the `philo-config` crate and produce exactly these two values.
    let provider = ProviderId::new("official-anthropic")?;
    let deployment = ProviderDeploymentConfig::new(
        provider.clone(),
        SecretReference::environment_variable("ANTHROPIC_API_KEY")?,
    );
    let runtime = ProviderRegistry::with_official_anthropic()?.build_deployment(
        &provider,
        &deployment,
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
