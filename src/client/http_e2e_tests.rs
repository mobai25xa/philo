//! Loopback HTTP coverage for the public client facade and stream Drop semantics.

use crate::provider::profiles::TestProvider;
use crate::test_http_server::{ResponsePlan, ScriptedServer};
use bytes::Bytes;
use http::StatusCode;
use philo::{GenerateRequest, LlmClient, Message, ModelRef, ReqwestTransport};
use serde_json::Value;

const API_KEY: &str = "philo-loopback-key-canary";

fn client(endpoint: &str) -> LlmClient {
    let runtime = TestProvider::localhost(endpoint, API_KEY)
        .unwrap()
        .build()
        .unwrap();
    LlmClient::new(runtime, ReqwestTransport::new().unwrap())
}

fn request(prompt: &str) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user(prompt)],
    )
}

#[tokio::test]
async fn loopback_chunked_sse_completes_through_public_client() {
    let sse = concat!(
        "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"好\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    )
    .as_bytes();
    let chunks = [0..17, 17..91, 91..92, 92..sse.len()]
        .into_iter()
        .map(|range| Bytes::copy_from_slice(&sse[range]));
    let server = ScriptedServer::spawn(vec![
        ResponsePlan::chunked(StatusCode::OK, chunks)
            .with_header("Content-Type", "text/event-stream")
            .with_header("X-Request-Id", "req-loopback"),
    ])
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let message = client(&endpoint)
        .complete(request("loopback prompt"))
        .await
        .unwrap();
    assert_eq!(message.text(), "你好");
    assert_eq!(
        message.provider_request_id().unwrap().as_str(),
        "req-loopback"
    );
    assert_eq!(message.generation_id().unwrap().as_str(), "gen-loopback");

    let result = server.finish().await;
    let inbound = &result.requests[0];
    assert_eq!(inbound.method(), "POST");
    assert_eq!(inbound.path(), "/v1/chat/completions");
    assert_eq!(
        inbound.header("authorization"),
        Some(format!("Bearer {API_KEY}").as_str())
    );
    assert_eq!(inbound.header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(inbound.body()).unwrap();
    assert_eq!(body["messages"][0]["content"], "loopback prompt");
}

#[tokio::test]
async fn dropping_public_stream_closes_incomplete_loopback_body() {
    let server = ScriptedServer::spawn(vec![
        ResponsePlan::chunked(StatusCode::OK, [Bytes::from_static(b": heartbeat\n\n")])
            .with_header("Content-Type", "text/event-stream")
            .incomplete(),
    ])
    .await;
    let endpoint = server.url("/v1/chat/completions");
    let stream = client(&endpoint)
        .stream(request("drop prompt"))
        .await
        .unwrap();
    server.wait_for_responses(1).await;
    drop(stream);
    assert_eq!(server.finish().await.disconnects, 1);
}
