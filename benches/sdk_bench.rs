//! Stable, offline SDK microbenchmark harness.

mod support;

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::domain::request::CapabilitySet;
use philo::observability::{LifecycleEvent, LifecycleObserver};
use philo::transport::{ByteStream, SseDecoder};
use philo::{GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef};

use support::mock_transport::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use support::performance::{PerformanceContext, load_budgets};
use support::provider::TestProvider;

const ENDPOINT: &str = "https://test.invalid/v1/chat/completions";

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
    .with_options(GenerationOptions::new().with_max_output_tokens(64))
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

fn anthropic_exchange() -> MockExchange {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"bench\",\"model\":\"gpt-test\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ))],
    ))
}

fn report(context: &PerformanceContext, name: &str, iterations: usize, elapsed: Duration) {
    let nanos_per_iteration = elapsed.as_nanos() / iterations.max(1) as u128;
    context.print_metric(
        name,
        "nanoseconds_per_iteration",
        nanos_per_iteration,
        iterations,
    );
}

async fn timed_client_call(
    client: &LlmClient,
    mock: &MockTransport,
    response: fn() -> MockExchange,
) -> Duration {
    mock.push(response());
    let started = Instant::now();
    black_box(client.complete(request()).await.unwrap());
    let elapsed = started.elapsed();
    mock.drain_captured_requests();
    elapsed
}

async fn run_async_benchmarks(
    context: &PerformanceContext,
    cpu_iterations: usize,
    async_iterations: usize,
) {
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
    report(context, "sse_decoder", cpu_iterations, started.elapsed());

    let provider = TestProvider::new(ENDPOINT, "benchmark-fixture-key")
        .unwrap()
        .build()
        .unwrap();
    let disabled_mock = MockTransport::default();
    let disabled = LlmClient::new(provider.clone(), disabled_mock.clone());
    let enabled_mock = MockTransport::default();
    let enabled =
        LlmClient::new(provider, enabled_mock.clone()).with_shared_observer(Arc::new(NoopObserver));

    for _ in 0..10 {
        timed_client_call(&disabled, &disabled_mock, exchange).await;
        timed_client_call(&enabled, &enabled_mock, exchange).await;
    }
    let mut disabled_elapsed = Duration::ZERO;
    let mut enabled_elapsed = Duration::ZERO;
    for iteration in 0..async_iterations {
        if iteration % 2 == 0 {
            disabled_elapsed += timed_client_call(&disabled, &disabled_mock, exchange).await;
            enabled_elapsed += timed_client_call(&enabled, &enabled_mock, exchange).await;
        } else {
            enabled_elapsed += timed_client_call(&enabled, &enabled_mock, exchange).await;
            disabled_elapsed += timed_client_call(&disabled, &disabled_mock, exchange).await;
        }
    }
    report(
        context,
        "openai_event_decode_hooks_disabled",
        async_iterations,
        disabled_elapsed,
    );
    report(
        context,
        "openai_event_decode_hooks_enabled",
        async_iterations,
        enabled_elapsed,
    );

    let anthropic_provider = TestProvider::new(ENDPOINT, "benchmark-fixture-key")
        .unwrap()
        .with_anthropic_messages()
        .build()
        .unwrap();
    let anthropic_mock = MockTransport::default();
    let anthropic = LlmClient::new(anthropic_provider, anthropic_mock.clone());
    for _ in 0..10 {
        timed_client_call(&anthropic, &anthropic_mock, anthropic_exchange).await;
    }
    let mut anthropic_elapsed = Duration::ZERO;
    for _ in 0..async_iterations {
        anthropic_elapsed +=
            timed_client_call(&anthropic, &anthropic_mock, anthropic_exchange).await;
    }
    report(
        context,
        "anthropic_event_decode",
        async_iterations,
        anthropic_elapsed,
    );
}

fn main() {
    let budgets = load_budgets();
    let context = PerformanceContext::new("microbenchmark", &budgets);
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    let cpu_iterations = if smoke {
        budgets.microbenchmark.smoke_cpu_iterations
    } else {
        budgets.microbenchmark.full_cpu_iterations
    };
    let async_iterations = if smoke {
        budgets.microbenchmark.smoke_async_iterations
    } else {
        budgets.microbenchmark.full_async_iterations
    };

    let fixed_request = request();
    let capabilities = CapabilitySet::default();
    let started = Instant::now();
    for _ in 0..cpu_iterations {
        fixed_request.validate(black_box(&capabilities)).unwrap();
    }
    report(
        &context,
        "request_validation",
        cpu_iterations,
        started.elapsed(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(run_async_benchmarks(
        &context,
        cpu_iterations,
        async_iterations,
    ));
}
