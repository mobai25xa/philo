//! Builds a multimodal HTTPS image request. Network I/O is optional.

mod support;

use philo::domain::content::{ImageContent, ImageDetail};
use philo::{ContentPart, GenerateRequest, Message, MessageRole, ModelRef};
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    // Synthetic non-user fixture URL. The SDK does not download it.
    let image =
        ImageContent::parse_url("https://example.com/fixtures/sample.png", ImageDetail::Auto)?;
    let message = Message::new(
        MessageRole::User,
        vec![
            ContentPart::text("Describe this sample image briefly."),
            ContentPart::Image(image),
        ],
    );
    println!("image debug stays metadata-only: {message:?}");

    if !support::has_live_credentials() {
        println!("offline image request construction ok");
        return Ok(());
    }

    let client = support::client_with_phase2_capabilities()?;
    let request = GenerateRequest::new(
        ModelRef::new("official-openai", std::env::var("OPENAI_MODEL")?)?,
        vec![message],
    );
    let response = client.complete(request).await?;
    println!("{}", response.text());
    Ok(())
}
