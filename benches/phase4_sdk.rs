//! Stable, offline Phase 4 SDK microbenchmark harness.

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::request::CapabilitySet;
use philo::observability::{LifecycleEvent, LifecycleObserver};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::transport::{ByteStream, SseDecoder};
use philo::{GenerateRequest, LlmClient, Message, ModelRef};

const ENDPOINT: &str = "http://127.0.0.1:41993/v1/chat/completions";

#[derive(Clone, Copy)]
struct NoopObserver;

impl LifecycleObserver for NoopObserver {
    fn record(&self, event: &LifecycleEvent) {
        black_box(event.elapsed());
    }
}

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("fixed benchmark fixture")],
    )
}

fn exchange() -> MockExchange {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(
            b"data: {\"id\":\"bench\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"bench\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ))],
    ))
}

fn report(name: &str, iterations: usize, elapsed: std::time::Duration) {
    let nanos_per_iteration = elapsed.as_nanos() / iterations.max(1) as u128;
    println!(
        "{{\"benchmark\":\"{name}\",\"iterations\":{iterations},\"elapsed_nanos\":{},\"nanos_per_iteration\":{nanos_per_iteration}}}",
        elapsed.as_nanos()
    );
}

async fn timed_client_call(client: &LlmClient, mock: &MockTransport) -> Duration {
    mock.push(exchange());
    let started = Instant::now();
    black_box(client.complete(request()).await.unwrap());
    let elapsed = started.elapsed();
    mock.drain_captured_requests();
    elapsed
}

fn main() {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    let cpu_iterations = if smoke { 1_000 } else { 25_000 };
    let async_iterations = if smoke { 50 } else { 1_000 };

    let fixed_request = request();
    let capabilities = CapabilitySet::default();
    let started = Instant::now();
    for _ in 0..cpu_iterations {
        fixed_request.validate(black_box(&capabilities)).unwrap();
    }
    report("request_validation", cpu_iterations, started.elapsed());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let payload = Bytes::from_static(b"data: {\"delta\":\"fixed\"}\n\ndata: [DONE]\n\n");
        let started = Instant::now();
        for _ in 0..cpu_iterations {
            let upstream: ByteStream = Box::pin(stream::once({
                let payload = payload.clone();
                async move { Ok(payload) }
            }));
            let mut decoder = SseDecoder::new(upstream);
            let mut count = 0_usize;
            while let Some(event) = decoder.next().await {
                black_box(event.unwrap());
                count += 1;
            }
            black_box(count);
        }
        report("sse_decoder", cpu_iterations, started.elapsed());

        let provider = TestOnlyProfile::localhost(ENDPOINT, "benchmark-fixture-key")
            .unwrap()
            .build()
            .unwrap();
        let disabled_mock = MockTransport::default();
        let disabled = LlmClient::new(provider.clone(), disabled_mock.clone());
        let enabled_mock = MockTransport::default();
        let enabled = LlmClient::new(provider, enabled_mock.clone())
            .with_shared_observer(Arc::new(NoopObserver));

        for _ in 0..10 {
            timed_client_call(&disabled, &disabled_mock).await;
            timed_client_call(&enabled, &enabled_mock).await;
        }
        let mut disabled_elapsed = Duration::ZERO;
        let mut enabled_elapsed = Duration::ZERO;
        for iteration in 0..async_iterations {
            if iteration % 2 == 0 {
                disabled_elapsed += timed_client_call(&disabled, &disabled_mock).await;
                enabled_elapsed += timed_client_call(&enabled, &enabled_mock).await;
            } else {
                enabled_elapsed += timed_client_call(&enabled, &enabled_mock).await;
                disabled_elapsed += timed_client_call(&disabled, &disabled_mock).await;
            }
        }
        report("client_hooks_disabled", async_iterations, disabled_elapsed);
        report("client_hooks_enabled", async_iterations, enabled_elapsed);
    });
}
