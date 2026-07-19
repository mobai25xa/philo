//! Streams official `OpenAI` text through the public domain event API.

mod support;

use futures_util::StreamExt as _;
use philo::AssistantEvent;
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let client = support::client()?;
    let mut stream = client
        .stream(support::request("Reply with one short sentence.")?)
        .await?;
    while let Some(event) = stream.next().await {
        if let AssistantEvent::TextDelta { delta, .. } = event? {
            print!("{delta}");
        }
    }
    println!();
    Ok(())
}
