//! Applies one overall timeout and caller-controlled cancellation.

mod support;

use std::time::Duration;

use futures_util::StreamExt as _;
use philo::{GenerationOptions, LlmError, RequestControl};
use support::ExampleResult;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult {
    let client = support::client()?;
    let options = GenerationOptions::new().with_timeout(Duration::from_secs(30))?;
    let request = support::request("Reply with one short sentence.")?.with_options(options);
    let control = RequestControl::new();
    let cancellation = control.clone();
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancellation.cancel();
    });

    match client.stream_with_control(request, control).await {
        Ok(mut stream) => {
            while let Some(item) = stream.next().await {
                if matches!(item, Err(LlmError::Cancelled)) {
                    break;
                }
                item?;
            }
        }
        Err(LlmError::Cancelled) => {}
        Err(error) => return Err(error.into()),
    }
    cancel_task.await?;
    Ok(())
}
