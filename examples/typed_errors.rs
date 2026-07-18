//! Handles stable public error categories without inspecting response bodies.

mod support;

use philo::LlmError;
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let client = support::client()?;
    match client.complete(support::request("Reply briefly.")?).await {
        Ok(message) => println!("{}", message.text()),
        Err(LlmError::Validation(error)) => eprintln!("invalid {}", error.field()),
        Err(LlmError::Capability(error)) => eprintln!("unsupported {}", error.capability()),
        Err(LlmError::HttpStatus(error)) => eprintln!("HTTP status {}", error.status()),
        Err(LlmError::Timeout(_)) => eprintln!("request timed out"),
        Err(LlmError::Cancelled) => eprintln!("request cancelled"),
        Err(error) => eprintln!("request failed in category: {error}"),
    }
    Ok(())
}
