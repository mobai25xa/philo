//! Loopback HTTP coverage for the public client facade and stream Drop semantics.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use bytes::Bytes;
use philo::provider::TestOnlyProfile;
use philo::{GenerateRequest, LlmClient, Message, ModelRef, ReqwestTransport};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::time::timeout;

const API_KEY: &str = "philo-loopback-key-canary";

struct InboundRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> std::io::Result<InboundRequest> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::other("request headers too large"));
        }
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::other("request ended before headers"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    };

    let text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let available = bytes.len().saturating_sub(header_end).min(content_length);
    Ok(InboundRequest {
        method,
        path,
        headers,
        body: Bytes::copy_from_slice(&bytes[header_end..header_end + available]),
    })
}

async fn write_chunk(stream: &mut tokio::net::TcpStream, chunk: &[u8]) -> std::io::Result<()> {
    let mut prefix = String::new();
    let _ = write!(prefix, "{:x}\r\n", chunk.len());
    stream.write_all(prefix.as_bytes()).await?;
    stream.write_all(chunk).await?;
    stream.write_all(b"\r\n").await
}

fn client(endpoint: &str) -> LlmClient {
    let runtime = TestOnlyProfile::localhost(endpoint, API_KEY)
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-Id: req-loopback\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let sse = concat!(
            "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"好\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"gen-loopback\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        )
        .as_bytes();
        for range in [0..17, 17..91, 91..92, 92..sse.len()] {
            write_chunk(&mut stream, &sse[range]).await.unwrap();
        }
        stream.write_all(b"0\r\n\r\n").await.unwrap();
        stream.shutdown().await.unwrap();
        request
    });

    let endpoint = format!("http://{address}/v1/chat/completions");
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

    let inbound = server.await.unwrap();
    assert_eq!(inbound.method, "POST");
    assert_eq!(inbound.path, "/v1/chat/completions");
    assert_eq!(
        inbound.headers["authorization"],
        format!("Bearer {API_KEY}")
    );
    assert_eq!(inbound.headers["content-type"], "application/json");
    let body: Value = serde_json::from_slice(&inbound.body).unwrap();
    assert_eq!(body["messages"][0]["content"], "loopback prompt");
}

#[tokio::test]
async fn dropping_public_stream_closes_incomplete_loopback_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\nf\r\n: heartbeat\n\nxx\r\n",
            )
            .await
            .unwrap();
        sent_tx.send(()).unwrap();
        let mut byte = [0_u8; 1];
        let closed = matches!(
            timeout(Duration::from_secs(2), stream.read(&mut byte)).await,
            Ok(Ok(0) | Err(_))
        );
        closed_tx.send(closed).unwrap();
    });

    let endpoint = format!("http://{address}/v1/chat/completions");
    let stream = client(&endpoint)
        .stream(request("drop prompt"))
        .await
        .unwrap();
    sent_rx.await.unwrap();
    drop(stream);
    assert!(closed_rx.await.unwrap());
    server.await.unwrap();
}
