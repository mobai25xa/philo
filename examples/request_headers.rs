//! Adds a non-sensitive, non-protected request header.

mod support;

use http::{HeaderName, HeaderValue};
use philo::GenerationOptions;
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let client = support::client()?;
    let options = GenerationOptions::new().with_header(
        HeaderName::from_static("x-philo-example-request"),
        HeaderValue::from_static("documentation"),
    );
    let request = support::request("Reply briefly.")?.with_options(options);
    let message = client.complete(request).await?;
    println!("{}", message.text());
    Ok(())
}
