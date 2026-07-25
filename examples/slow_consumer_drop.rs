//! Demonstrates explicit cancellation/drop without buffering a response in the SDK.

mod support;

use futures_util::StreamExt as _;
use philo::{GenerateRequest, LlmClient, Message, RequestControl};

#[tokio::main(flavor = "current_thread")]
async fn main() -> support::ExampleResult {
    let runtime = support::official_runtime_from_env()?;
    let client = LlmClient::with_reqwest(runtime)?;
    let control = RequestControl::new();
    let cancel = control.clone();
    let request = GenerateRequest::new(
        support::model_from_env("official-openai")?,
        vec![Message::user("Reply briefly.")],
    );
    let mut stream = client.stream_with_control(request, control).await?;
    if let Some(event) = stream.next().await {
        let _ = event?;
        cancel.cancel();
    }
    drop(stream);
    Ok(())
}
