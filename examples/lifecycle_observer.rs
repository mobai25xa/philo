//! Installs a non-blocking value-free lifecycle observer.

mod support;

use philo::{
    GenerateRequest, LifecycleEvent, LifecycleEventKind, LifecycleObserver, LlmClient, Message,
};

#[derive(Clone, Copy)]
struct SafeObserver;

impl LifecycleObserver for SafeObserver {
    fn record(&self, event: &LifecycleEvent) {
        let name = match event.kind() {
            LifecycleEventKind::RequestStarted => "request.started",
            LifecycleEventKind::AttemptStarted { .. } => "attempt.started",
            LifecycleEventKind::RetryScheduled { .. } => "retry.scheduled",
            LifecycleEventKind::RateLimitObserved { .. } => "rate_limit.observed",
            LifecycleEventKind::RequestCompleted { .. } => "request.completed",
            LifecycleEventKind::RequestFailed { .. } => "request.failed",
            LifecycleEventKind::RequestCancelled { .. } => "request.cancelled",
            LifecycleEventKind::RequestTimedOut { .. } => "request.timed_out",
            _ => "request.progress",
        };
        eprintln!("{name} elapsed_ms={}", event.elapsed().as_millis());
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> support::ExampleResult {
    let runtime = support::official_runtime_from_env()?;
    let client = LlmClient::with_reqwest(runtime)?.with_observer(SafeObserver);
    let request = GenerateRequest::new(
        support::model_from_env("official-openai")?,
        vec![Message::user("Reply briefly.")],
    );
    let message = client.complete(request).await?;
    println!("{}", message.text());
    Ok(())
}
