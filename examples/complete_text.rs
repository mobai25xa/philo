//! Collects one official `OpenAI` text stream into an assistant message.

mod support;

use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let client = support::client()?;
    let message = client
        .complete(support::request("Reply with one short sentence.")?)
        .await?;
    println!("{}", message.text());
    Ok(())
}
