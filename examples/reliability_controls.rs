//! Configures bounded Phase 4 timeout, retry, wait, cancellation, and idempotency controls.

mod support;

use std::time::Duration;

use philo::{
    GenerateRequest, LlmClient, Message, RequestControl, RetryPolicy, RetryWaitPolicy,
    TimeoutPolicy,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> support::ExampleResult {
    let runtime = support::official_runtime_from_env()?;
    let timeouts = TimeoutPolicy::new()
        .with_response_header_timeout(Duration::from_secs(15))?
        .with_first_event_timeout(Duration::from_secs(20))?
        .with_idle_stream_timeout(Duration::from_secs(45))?;
    let retries = RetryPolicy::standard()
        .with_max_attempts(3)?
        .with_minimum_attempt_budget(Duration::from_millis(250))?;
    let waits = RetryWaitPolicy::new()
        .with_max_delay(Duration::from_secs(2))?
        .with_server_delay_cap(Duration::from_secs(30))?
        .with_max_total_wait(Duration::from_secs(45))?;
    let client = LlmClient::with_reqwest(runtime)?
        .with_timeout_policy(timeouts)
        .with_retry_policy(retries)
        .with_retry_wait_policy(waits);
    let control = RequestControl::new().with_generated_idempotency_key();
    let request = GenerateRequest::new(
        support::model_from_env("official-openai")?,
        vec![Message::user("Reply briefly.")],
    );
    let message = client.complete_with_control(request, control).await?;
    println!("{}", message.text());
    Ok(())
}
