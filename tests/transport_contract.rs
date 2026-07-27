//! Shared mock/reqwest transport contract and loopback lifecycle coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as _;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use philo::domain::ids::LocalRequestId;
use philo::error::{ErrorStage, RetriableHint, TransportError};
use philo::provider::TestOnlyProfile;
use philo::provider::endpoint::RedirectPolicy;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::transport::read_body_limited;
use philo::transport::{CancellationToken, HttpRequest, RequestLifecycle, TransportContext};
use philo::{LlmError, ReqwestTransport, Transport};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

const CANARY: &str = "philo-transport-canary-secret";

#[derive(Clone, Debug)]
struct InboundRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

#[derive(Clone)]
struct ResponsePlan {
    status: StatusCode,
    headers: Vec<(String, String)>,
    start_delay: Duration,
    chunks: Vec<(Duration, Bytes)>,
    declared_length: Option<usize>,
    complete: bool,
}

impl ResponsePlan {
    fn chunked(status: StatusCode, chunks: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            start_delay: Duration::ZERO,
            chunks: chunks
                .into_iter()
                .map(|chunk| (Duration::ZERO, chunk))
                .collect(),
            declared_length: None,
            complete: true,
        }
    }

    fn fixed(status: StatusCode, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        Self {
            status,
            headers: Vec::new(),
            start_delay: Duration::ZERO,
            declared_length: Some(body.len()),
            chunks: vec![(Duration::ZERO, body)],
            complete: true,
        }
    }

    fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }

    fn with_start_delay(mut self, delay: Duration) -> Self {
        self.start_delay = delay;
        self
    }

    fn with_chunk_delays(mut self, delays: impl IntoIterator<Item = Duration>) -> Self {
        for ((delay, _), replacement) in self.chunks.iter_mut().zip(delays) {
            *delay = replacement;
        }
        self
    }

    fn incomplete_content_length(body: impl Into<Bytes>, declared_length: usize) -> Self {
        Self {
            status: StatusCode::OK,
            headers: Vec::new(),
            start_delay: Duration::ZERO,
            chunks: vec![(Duration::ZERO, body.into())],
            declared_length: Some(declared_length),
            complete: false,
        }
    }
}

struct RunningServer {
    address: std::net::SocketAddr,
    captured: Arc<Mutex<Vec<InboundRequest>>>,
    task: JoinHandle<()>,
}

impl RunningServer {
    async fn spawn(plans: Vec<ResponsePlan>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        Self::spawn_bound(listener, address, plans)
    }

    async fn spawn_with(plans: impl FnOnce(std::net::SocketAddr) -> Vec<ResponsePlan>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        Self::spawn_bound(listener, address, plans(address))
    }

    fn spawn_bound(
        listener: TcpListener,
        address: std::net::SocketAddr,
        plans: Vec<ResponsePlan>,
    ) -> Self {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let task_captured = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            let mut handlers = Vec::with_capacity(plans.len());
            for plan in plans {
                let (stream, _) = listener.accept().await.unwrap();
                let captured = Arc::clone(&task_captured);
                handlers.push(tokio::spawn(async move {
                    handle_connection(stream, plan, captured).await;
                }));
            }
            for handler in handlers {
                handler.await.unwrap();
            }
        });
        Self {
            address,
            captured,
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    async fn finish(self) -> Vec<InboundRequest> {
        timeout(Duration::from_secs(3), self.task)
            .await
            .expect("server did not finish")
            .unwrap();
        lock(&self.captured).clone()
    }

    async fn finish_after_client_stop(mut self) -> Vec<InboundRequest> {
        if timeout(Duration::from_secs(3), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = self.task.await;
        }
        lock(&self.captured).clone()
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    plan: ResponsePlan,
    captured: Arc<Mutex<Vec<InboundRequest>>>,
) {
    let request = read_inbound(&mut stream).await.unwrap();
    lock(&captured).push(request);
    sleep(plan.start_delay).await;

    let reason = plan.status.canonical_reason().unwrap_or("Unknown");
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", plan.status.as_u16());
    for (name, value) in &plan.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if let Some(length) = plan.declared_length {
        let _ = write!(head, "Content-Length: {length}\r\n");
    } else {
        head.push_str("Transfer-Encoding: chunked\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }

    for (delay, chunk) in plan.chunks {
        sleep(delay).await;
        let result = if plan.declared_length.is_some() {
            stream.write_all(&chunk).await
        } else {
            let prefix = format!("{:x}\r\n", chunk.len());
            if stream.write_all(prefix.as_bytes()).await.is_err() {
                return;
            }
            if stream.write_all(&chunk).await.is_err() {
                return;
            }
            stream.write_all(b"\r\n").await
        };
        if result.is_err() {
            return;
        }
    }
    if plan.complete && plan.declared_length.is_none() {
        let _ = stream.write_all(b"0\r\n\r\n").await;
    }
    let _ = stream.shutdown().await;
}

async fn read_inbound(stream: &mut TcpStream) -> std::io::Result<InboundRequest> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
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

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn request(
    url: &str,
    id: &str,
    body: impl Into<Bytes>,
    request_headers: impl std::borrow::Borrow<HeaderMap>,
    lifecycle: RequestLifecycle,
    redirect_policy: RedirectPolicy,
) -> HttpRequest {
    let runtime = TestOnlyProfile::localhost(url, CANARY)
        .unwrap()
        .build()
        .unwrap();
    let headers = runtime
        .resolve_headers(Vec::new(), request_headers.borrow())
        .unwrap()
        .headers()
        .clone();
    HttpRequest::new(
        runtime.method(),
        runtime.endpoint().clone(),
        headers,
        body.into(),
        TransportContext::new(LocalRequestId::new(id).unwrap()),
    )
    .with_lifecycle(lifecycle)
    .with_redirect_policy(redirect_policy)
}

async fn assert_common_contract<T: Transport>(transport: &T, request: HttpRequest) -> Vec<u8> {
    let response = transport.execute(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers().get("x-philo-contract").unwrap(), "yes");
    let mut body = response.into_body();
    let mut collected = Vec::new();
    while let Some(item) = body.next().await {
        collected.extend_from_slice(&item.unwrap());
    }
    assert_eq!(collected, b"alphabeta");
    collected
}

#[tokio::test]
async fn mock_and_reqwest_share_the_http_byte_contract() {
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        HeaderName::from_static("x-philo-contract"),
        HeaderValue::from_static("yes"),
    );
    let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::ACCEPTED,
        response_headers,
        vec![MockBodyItem::chunk("alpha"), MockBodyItem::chunk("beta")],
    ))]);
    let mock_url = "http://127.0.0.1:41001/v1/chat/completions";
    assert_common_contract(
        &mock,
        request(
            mock_url,
            "mock-common",
            Bytes::from_static(br#"{"mock":true}"#),
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ),
    )
    .await;
    mock.assert_consumed();
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method(), http::Method::POST);
    assert_eq!(captured[0].body(), br#"{"mock":true}"#.as_slice());

    let server = RunningServer::spawn(vec![
        ResponsePlan::chunked(
            StatusCode::ACCEPTED,
            [Bytes::from_static(b"alpha"), Bytes::from_static(b"beta")],
        )
        .with_header("X-Philo-Contract", "yes"),
    ])
    .await;
    let url = server.url("/v1/chat/completions");
    let transport = ReqwestTransport::new().unwrap();
    assert_common_contract(
        &transport,
        request(
            &url,
            "reqwest-common",
            Bytes::from_static(br#"{"real":true}"#),
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ),
    )
    .await;
    let inbound = server.finish().await;
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].method, "POST");
    assert_eq!(inbound[0].path, "/v1/chat/completions");
    assert_eq!(inbound[0].body, br#"{"real":true}"#.as_slice());
    assert_eq!(inbound[0].headers["content-type"], "application/json");
    assert_eq!(
        inbound[0].headers["authorization"],
        format!("Bearer {CANARY}")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn mock_distinguishes_start_and_body_errors_and_observes_lifecycle() {
    let mock = MockTransport::scripted([
        MockExchange::start_error(
            TransportError::new(ErrorStage::Connect, RetriableHint::Maybe).into(),
        ),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            vec![
                MockBodyItem::chunk("first"),
                MockBodyItem::error(
                    TransportError::new(ErrorStage::Body, RetriableHint::Maybe).into(),
                ),
            ],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            vec![MockBodyItem::delayed_chunk(Duration::from_secs(1), "late")],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            vec![
                MockBodyItem::delayed_chunk(Duration::from_secs(1), "drop"),
                MockBodyItem::chunk("unread"),
            ],
        )),
    ]);
    let url = "http://127.0.0.1:41002/v1/chat/completions";

    let start = mock
        .execute(request(
            url,
            "mock-start-error",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await;
    assert!(matches!(
        start,
        Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Connect
    ));

    let mut body = mock
        .execute(request(
            url,
            "mock-body-error",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    assert_eq!(body.next().await.unwrap().unwrap(), "first");
    assert!(matches!(
        body.next().await.unwrap(),
        Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Body
    ));

    let cancellation = CancellationToken::new();
    let mut cancellable_body = mock
        .execute(request(
            url,
            "mock-body-cancel",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::new(cancellation.clone()),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    cancellation.cancel();
    assert!(matches!(
        cancellable_body.next().await.unwrap(),
        Err(LlmError::Cancelled)
    ));
    assert_eq!(mock.body_cancellation_count(), 1);

    let dropped = mock
        .execute(request(
            url,
            "mock-body-drop",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    drop(dropped);
    assert_eq!(mock.early_body_drop_count(), 1);
    mock.assert_consumed();

    let pre_cancelled = CancellationToken::new();
    pre_cancelled.cancel();
    let untouched = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::OK,
        HeaderMap::new(),
        Vec::new(),
    ))]);
    let result = untouched
        .execute(request(
            url,
            "mock-pre-cancel",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::new(pre_cancelled),
            RedirectPolicy::Disabled,
        ))
        .await;
    assert!(matches!(result, Err(LlmError::Cancelled)));
    assert_eq!(untouched.remaining_expectations(), 1);
    assert!(untouched.captured_requests().is_empty());
}

#[tokio::test]
async fn mock_concurrency_is_correlated_by_local_request_id() {
    const COUNT: usize = 12;
    let mock = MockTransport::scripted((0..COUNT).map(|_| {
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            vec![MockBodyItem::chunk("ok")],
        ))
    }));
    let mut tasks = Vec::new();
    for index in 0..COUNT {
        let transport = mock.clone();
        tasks.push(tokio::spawn(async move {
            let id = format!("mock-concurrent-{index}");
            let response = transport
                .execute(request(
                    "http://127.0.0.1:41003/v1/chat/completions",
                    &id,
                    id.clone(),
                    HeaderMap::new(),
                    RequestLifecycle::default(),
                    RedirectPolicy::Disabled,
                ))
                .await
                .unwrap();
            let body = read_body_limited(response.into_body(), 16).await.unwrap();
            assert_eq!(body.bytes(), b"ok".as_slice());
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    mock.assert_consumed();
    let ids: BTreeSet<_> = mock
        .captured_requests()
        .into_iter()
        .map(|request| request.local_request_id().as_str().to_owned())
        .collect();
    let expected: BTreeSet<_> = (0..COUNT)
        .map(|index| format!("mock-concurrent-{index}"))
        .collect();
    assert_eq!(ids, expected);
}

#[tokio::test]
async fn bounded_body_reader_stops_at_limit_and_redacts_binary_secrets() {
    let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::BAD_REQUEST,
        HeaderMap::new(),
        vec![
            MockBodyItem::chunk(Bytes::from_static(b"api_key=sk-canary")),
            MockBodyItem::chunk(Bytes::from_static(&[0xff, b'x'])),
        ],
    ))]);
    let response = mock
        .execute(request(
            "http://127.0.0.1:41004/v1/chat/completions",
            "bounded-body",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap();
    let body = read_body_limited(response.into_body(), 8).await.unwrap();
    assert_eq!(body.bytes().len(), 8);
    assert!(body.is_truncated());
    assert_eq!(
        body.summary().as_str(),
        "<redacted error body>... [truncated]"
    );
}

#[tokio::test]
async fn reqwest_preserves_error_statuses_and_classifies_connect_and_body_failures() {
    let server = RunningServer::spawn(vec![
        ResponsePlan::fixed(StatusCode::UNAUTHORIZED, "unauthorized"),
        ResponsePlan::fixed(StatusCode::TOO_MANY_REQUESTS, "limited"),
        ResponsePlan::fixed(StatusCode::INTERNAL_SERVER_ERROR, "server"),
        ResponsePlan::incomplete_content_length("abc", 10),
    ])
    .await;
    let url = server.url("/v1/chat/completions");
    let transport = ReqwestTransport::new().unwrap();
    for (index, expected) in [
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ]
    .into_iter()
    .enumerate()
    {
        let response = transport
            .execute(request(
                &url,
                &format!("status-{index}"),
                "{}",
                HeaderMap::new(),
                RequestLifecycle::default(),
                RedirectPolicy::Disabled,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(!response.headers().is_empty());
    }

    let mut body = transport
        .execute(request(
            &url,
            "body-disconnect",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    assert_eq!(body.next().await.unwrap().unwrap(), "abc");
    assert!(matches!(
        body.next().await.unwrap(),
        Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Body
    ));
    server.finish().await;

    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = unused.local_addr().unwrap();
    drop(unused);
    let connect = transport
        .execute(request(
            &format!("http://{address}/v1/chat/completions"),
            "connect-failure",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await;
    assert!(matches!(
        &connect,
        Err(LlmError::Transport(error)) if error.stage() == ErrorStage::Connect
    ));
    if let Err(LlmError::Transport(error)) = &connect {
        let source = error.source().unwrap();
        assert!(!source.to_string().contains(&address.to_string()));
        assert!(!source.to_string().contains("reqwest::Error"));
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn reqwest_classifies_tls_handshake_failures_separately() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut hello = [0_u8; 2048];
        let _ = stream.read(&mut hello).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
    });
    let transport = ReqwestTransport::new().unwrap();
    let result = transport
        .execute(request(
            &format!("https://{address}/v1/chat/completions"),
            "tls-failure",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await;
    assert!(
        matches!(
            result,
            Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Tls
        ),
        "unexpected TLS result: {result:?}"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn reqwest_propagates_cancel_deadline_and_body_cancel() {
    let transport = ReqwestTransport::new().unwrap();

    let cancellation_server = RunningServer::spawn(vec![
        ResponsePlan::fixed(StatusCode::OK, "late").with_start_delay(Duration::from_millis(200)),
    ])
    .await;
    let cancellation_url = cancellation_server.url("/v1/chat/completions");
    let cancellation = CancellationToken::new();
    let transport_task = transport.clone();
    let cancellation_task = cancellation.clone();
    let cancelled = tokio::spawn(async move {
        transport_task
            .execute(request(
                &cancellation_url,
                "cancel-before-headers",
                "{}",
                HeaderMap::new(),
                RequestLifecycle::new(cancellation_task),
                RedirectPolicy::Disabled,
            ))
            .await
    });
    sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    assert!(matches!(cancelled.await.unwrap(), Err(LlmError::Cancelled)));
    let cancellation_requests = cancellation_server.finish_after_client_stop().await;
    assert!(cancellation_requests.len() <= 1);

    let deadline_server = RunningServer::spawn(vec![
        ResponsePlan::fixed(StatusCode::OK, "late").with_start_delay(Duration::from_millis(200)),
    ])
    .await;
    let deadline = transport
        .execute(request(
            &deadline_server.url("/v1/chat/completions"),
            "deadline-before-headers",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default().with_deadline(Instant::now() + Duration::from_millis(20)),
            RedirectPolicy::Disabled,
        ))
        .await;
    assert!(matches!(deadline, Err(LlmError::Timeout(_))));
    let deadline_requests = deadline_server.finish_after_client_stop().await;
    assert!(deadline_requests.len() <= 1);

    let body_server = RunningServer::spawn(vec![
        ResponsePlan::chunked(
            StatusCode::OK,
            [Bytes::from_static(b"first"), Bytes::from_static(b"second")],
        )
        .with_chunk_delays([Duration::ZERO, Duration::from_millis(200)]),
    ])
    .await;
    let body_cancellation = CancellationToken::new();
    let mut body = transport
        .execute(request(
            &body_server.url("/v1/chat/completions"),
            "cancel-body",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::new(body_cancellation.clone()),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    assert_eq!(body.next().await.unwrap().unwrap(), "first");
    body_cancellation.cancel();
    assert!(matches!(
        body.next().await.unwrap(),
        Err(LlmError::Cancelled)
    ));
    assert_eq!(body_server.finish_after_client_stop().await.len(), 1);
}

#[tokio::test]
async fn reqwest_drop_body_closes_the_incomplete_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_inbound(&mut stream).await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n5\r\nfirst\r\n",
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

    let transport = ReqwestTransport::new().unwrap();
    let mut body = transport
        .execute(request(
            &format!("http://{address}/v1/chat/completions"),
            "drop-body",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap()
        .into_body();
    sent_rx.await.unwrap();
    assert_eq!(body.next().await.unwrap().unwrap(), "first");
    drop(body);
    assert!(closed_rx.await.unwrap());
    server.await.unwrap();
}

#[tokio::test]
async fn reqwest_concurrent_headers_do_not_cross_talk() {
    const COUNT: usize = 10;
    let server = RunningServer::spawn(
        (0..COUNT)
            .map(|_| ResponsePlan::fixed(StatusCode::OK, "ok"))
            .collect(),
    )
    .await;
    let url = server.url("/v1/chat/completions");
    let transport = ReqwestTransport::new().unwrap();
    let mut tasks = Vec::new();
    for index in 0..COUNT {
        let transport = transport.clone();
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let marker = format!("request-{index}");
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_static("x-philo-request"),
                HeaderValue::from_str(&marker).unwrap(),
            );
            let response = transport
                .execute(request(
                    &url,
                    &format!("concurrent-{index}"),
                    marker.clone(),
                    headers,
                    RequestLifecycle::default(),
                    RedirectPolicy::Disabled,
                ))
                .await
                .unwrap();
            read_body_limited(response.into_body(), 16).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let inbound = server.finish().await;
    assert_eq!(inbound.len(), COUNT);
    for request in inbound {
        let marker = String::from_utf8(request.body.to_vec()).unwrap();
        assert_eq!(request.headers["x-philo-request"], marker);
        assert_eq!(request.headers["authorization"], format!("Bearer {CANARY}"));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn reqwest_redirect_policy_disables_cross_origin_and_limits_hops() {
    let disabled_server = RunningServer::spawn(vec![
        ResponsePlan::fixed(StatusCode::FOUND, Bytes::new()).with_header("Location", "/next"),
    ])
    .await;
    let disabled_url = disabled_server.url("/start");
    let transport = ReqwestTransport::new().unwrap();
    let disabled = transport
        .execute(request(
            &disabled_url,
            "redirect-disabled",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::Disabled,
        ))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::FOUND);
    assert_eq!(disabled_server.finish().await.len(), 1);

    let same_origin_server = RunningServer::spawn_with(|address| {
        vec![
            ResponsePlan::fixed(StatusCode::FOUND, Bytes::new())
                .with_header("Location", format!("http://{address}/next")),
            ResponsePlan::fixed(StatusCode::OK, "followed"),
        ]
    })
    .await;
    let same_origin_url = same_origin_server.url("/start");
    let same_origin = transport
        .execute(request(
            &same_origin_url,
            "redirect-same-origin",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::SameOrigin,
        ))
        .await
        .unwrap();
    assert_eq!(same_origin.status(), StatusCode::OK);
    assert_eq!(
        read_body_limited(same_origin.into_body(), 32)
            .await
            .unwrap()
            .bytes(),
        b"followed".as_slice()
    );
    let same_origin_captured = same_origin_server.finish().await;
    assert_eq!(same_origin_captured.len(), 2);
    assert_eq!(same_origin_captured[1].path, "/next");
    assert_eq!(
        same_origin_captured[1].headers["authorization"],
        format!("Bearer {CANARY}")
    );

    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_address = target.local_addr().unwrap();
    let cross_server = RunningServer::spawn(vec![
        ResponsePlan::fixed(StatusCode::FOUND, Bytes::new())
            .with_header("Location", format!("http://{target_address}/stolen")),
    ])
    .await;
    let cross = transport
        .execute(request(
            &cross_server.url("/start"),
            "redirect-cross-origin",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::SameOrigin,
        ))
        .await;
    assert!(matches!(
        cross,
        Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Http
    ));
    assert!(
        timeout(Duration::from_millis(150), target.accept())
            .await
            .is_err()
    );
    cross_server.finish().await;

    let redirect_loop = RunningServer::spawn(
        (0..=5)
            .map(|_| {
                ResponsePlan::fixed(StatusCode::FOUND, Bytes::new())
                    .with_header("Location", "/loop")
            })
            .collect(),
    )
    .await;
    let loop_result = transport
        .execute(request(
            &redirect_loop.url("/loop"),
            "redirect-limit",
            "{}",
            HeaderMap::new(),
            RequestLifecycle::default(),
            RedirectPolicy::SameOrigin,
        ))
        .await;
    assert!(matches!(
        loop_result,
        Err(LlmError::Transport(ref error)) if error.stage() == ErrorStage::Http
    ));
    assert_eq!(redirect_loop.finish().await.len(), 6);
}

#[test]
fn request_and_response_debug_are_value_free() {
    let secret_body = Bytes::from_static(b"prompt-canary-secret");
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-philo-secret"),
        HeaderValue::from_static("header-canary-secret"),
    );
    let request = request(
        "http://127.0.0.1:41005/v1/chat/completions",
        "debug-safe",
        secret_body,
        headers,
        RequestLifecycle::default(),
        RedirectPolicy::Disabled,
    );
    let debug = format!("{request:?}");
    assert!(!debug.contains("prompt-canary-secret"));
    assert!(!debug.contains("header-canary-secret"));
    assert!(!debug.contains(CANARY));
    assert!(debug.contains("x-philo-secret"));
}
