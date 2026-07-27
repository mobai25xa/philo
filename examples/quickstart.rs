//! Sends one streaming request using only the crate-root first-request API.

mod support;

use futures_util::StreamExt as _;
use philo::{AssistantEvent, GenerateRequest, LlmClient, Message, ModelRef};
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    if !support::has_live_credentials() {
        println!("set OPENAI_API_KEY and OPENAI_MODEL to run the request");
        return Ok(());
    }

    let client: LlmClient = support::client()?;
    let request = GenerateRequest::new(
        ModelRef::new("official-openai", std::env::var("OPENAI_MODEL")?)?,
        vec![Message::user("Reply with one short sentence.")],
    );
    let mut stream = client.stream(request).await?;
    while let Some(event) = stream.next().await {
        if let AssistantEvent::TextDelta { delta, .. } = event? {
            print!("{delta}");
        }
    }
    println!();
    Ok(())
}
