//! P1-018 reusable loopback script-server integration coverage.

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use philo::provider::TestOnlyProfile;
use philo::transport::ReqwestTransport;
use philo::{GenerateRequest, LlmClient, LlmError, Message, ModelRef};
use serde_json::Value;
use support::http_server::{ResponsePlan, ScriptedServer};

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("fixture prompt")],
    )
}

fn sse(generation_id: &str, text: &str) -> Bytes {
    Bytes::from(format!(
        "data: {{\"id\":\"{generation_id}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"{text}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"{generation_id}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
    ))
}

fn chunks(bytes: &[u8]) -> Vec<Bytes> {
    bytes.chunks(11).map(Bytes::copy_from_slice).collect()
}

fn runtime(endpoint: &str) -> philo::ProviderRuntime {
    TestOnlyProfile::localhost(endpoint, "release-contract-key")
        .unwrap()
        .build()
        .unwrap()
}

#[tokio::test]
async fn reusable_loopback_server_asserts_request_and_arbitrary_chunks() {
    let payload = sse("generation-release", "hello");
    let server = ScriptedServer::spawn(vec![
        ResponsePlan::chunked(StatusCode::OK, chunks(&payload))
            .with_header("Content-Type", "text/event-stream; charset=utf-8")
            .with_header("X-Request-Id", "provider-release")
            .with_accept_delay(Duration::from_millis(1))
            .with_start_delay(Duration::from_millis(1)),
    ])
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let client = LlmClient::new(runtime(&endpoint), ReqwestTransport::new().unwrap());
    let message = client.complete(request()).await.unwrap();
    assert_eq!(message.text(), "hello");
    assert_eq!(
        message.provider_request_id().unwrap().as_str(),
        "provider-release"
    );
    assert_eq!(
        message.generation_id().unwrap().as_str(),
        "generation-release"
    );

    let result = server.finish().await;
    assert_eq!(result.disconnects, 0);
    assert_eq!(result.requests.len(), 1);
    let inbound = &result.requests[0];
    assert_eq!(inbound.method(), "POST");
    assert_eq!(inbound.path(), "/v1/chat/completions");
    assert_eq!(
        inbound.header("authorization"),
        Some("Bearer release-contract-key")
    );
    assert_eq!(inbound.header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(inbound.body()).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(body["n"], 1);
    assert_eq!(body["messages"][0]["content"], "fixture prompt");
}

#[tokio::test]
async fn reusable_loopback_server_covers_bounded_error_and_disconnect() {
    let server = ScriptedServer::spawn(vec![
        ResponsePlan::fixed(
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"message":"synthetic rate limit"}}"#,
        )
        .with_header("Content-Type", "application/json")
        .with_header("X-Request-Id", "provider-error"),
    ])
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let client = LlmClient::new(runtime(&endpoint), ReqwestTransport::new().unwrap());
    let error = client.stream(request()).await.unwrap_err();
    assert!(matches!(error, LlmError::HttpStatus(ref error) if error.status() == 429));
    let result = server.finish().await;
    assert_eq!(result.disconnects, 0);

    let server = ScriptedServer::spawn(vec![
        ResponsePlan::chunked(StatusCode::OK, [Bytes::from_static(b": heartbeat\n\n")])
            .with_header("Content-Type", "text/event-stream")
            .incomplete(),
    ])
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let client = LlmClient::new(runtime(&endpoint), ReqwestTransport::new().unwrap());
    let stream = client.stream(request()).await.unwrap();
    server.wait_for_responses(1).await;
    drop(stream);
    let result = server.finish().await;
    assert_eq!(result.disconnects, 1);
}

#[tokio::test]
async fn reusable_loopback_server_handles_concurrent_requests_without_port_collisions() {
    let server = ScriptedServer::spawn(
        (0_u64..4)
            .map(|index| {
                ResponsePlan::chunked(
                    StatusCode::OK,
                    chunks(&sse(&format!("generation-{index}"), "ok")),
                )
                .with_header("Content-Type", "text/event-stream")
                .with_header("X-Request-Id", format!("provider-{index}"))
                .with_chunk_delays([Duration::from_millis(index)])
            })
            .collect(),
    )
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let client = Arc::new(LlmClient::new(
        runtime(&endpoint),
        ReqwestTransport::new().unwrap(),
    ));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let client = Arc::clone(&client);
        tasks.push(tokio::spawn(async move {
            client.complete(request()).await.unwrap()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().text(), "ok");
    }
    let result = server.finish().await;
    assert_eq!(result.requests.len(), 4);
}
