//! Small loopback-only HTTP script server for offline contract tests.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug)]
pub struct CapturedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

impl CapturedRequest {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body(&self) -> &Bytes {
        &self.body
    }
}

#[derive(Clone)]
pub struct ResponsePlan {
    status: StatusCode,
    headers: Vec<(String, String)>,
    chunks: Vec<(Duration, Bytes)>,
    declared_length: Option<usize>,
    complete: bool,
    start_delay: Duration,
    accept_delay: Duration,
    observe_disconnect: bool,
}

impl ResponsePlan {
    pub fn fixed(status: StatusCode, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        Self {
            status,
            headers: Vec::new(),
            chunks: vec![(Duration::ZERO, body.clone())],
            declared_length: Some(body.len()),
            complete: true,
            start_delay: Duration::ZERO,
            accept_delay: Duration::ZERO,
            observe_disconnect: false,
        }
    }

    pub fn chunked(status: StatusCode, chunks: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            chunks: chunks
                .into_iter()
                .map(|chunk| (Duration::ZERO, chunk))
                .collect(),
            declared_length: None,
            complete: true,
            start_delay: Duration::ZERO,
            accept_delay: Duration::ZERO,
            observe_disconnect: false,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_start_delay(mut self, delay: Duration) -> Self {
        self.start_delay = delay;
        self
    }

    pub fn with_accept_delay(mut self, delay: Duration) -> Self {
        self.accept_delay = delay;
        self
    }

    pub fn with_chunk_delays(mut self, delays: impl IntoIterator<Item = Duration>) -> Self {
        for ((delay, _), replacement) in self.chunks.iter_mut().zip(delays) {
            *delay = replacement;
        }
        self
    }

    pub fn incomplete(mut self) -> Self {
        self.complete = false;
        self.observe_disconnect = true;
        self
    }
}

pub struct ScriptedServer {
    address: std::net::SocketAddr,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    disconnects: Arc<AtomicUsize>,
    responses_started: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

pub struct ServerResult {
    pub requests: Vec<CapturedRequest>,
    pub disconnects: usize,
}

impl ScriptedServer {
    pub async fn spawn(plans: Vec<ResponsePlan>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let disconnects = Arc::new(AtomicUsize::new(0));
        let responses_started = Arc::new(AtomicUsize::new(0));
        let task_captured = Arc::clone(&captured);
        let task_disconnects = Arc::clone(&disconnects);
        let task_responses_started = Arc::clone(&responses_started);
        let task = tokio::spawn(async move {
            let mut handlers = Vec::with_capacity(plans.len());
            for plan in plans {
                sleep(plan.accept_delay).await;
                let (stream, _) = listener.accept().await.unwrap();
                let captured = Arc::clone(&task_captured);
                let disconnects = Arc::clone(&task_disconnects);
                let responses_started = Arc::clone(&task_responses_started);
                handlers.push(tokio::spawn(async move {
                    handle_connection(stream, plan, captured, disconnects, responses_started).await;
                }));
            }
            for handler in handlers {
                handler.await.unwrap();
            }
        });
        Self {
            address,
            captured,
            disconnects,
            responses_started,
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    pub async fn wait_for_responses(&self, count: usize) {
        timeout(Duration::from_secs(2), async {
            while self.responses_started.load(Ordering::SeqCst) < count {
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("scripted responses were not sent");
    }

    pub async fn finish(self) -> ServerResult {
        timeout(Duration::from_secs(3), self.task)
            .await
            .expect("scripted server did not finish")
            .expect("scripted server task failed");
        ServerResult {
            requests: lock(&self.captured).clone(),
            disconnects: self.disconnects.load(Ordering::SeqCst),
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    plan: ResponsePlan,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    disconnects: Arc<AtomicUsize>,
    responses_started: Arc<AtomicUsize>,
) {
    let request = read_request(&mut stream).await.unwrap();
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
    if plan.complete {
        head.push_str("Connection: close\r\n\r\n");
    } else {
        head.push_str("Connection: keep-alive\r\n\r\n");
    }
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }

    for (delay, chunk) in plan.chunks {
        sleep(delay).await;
        if let Some(_length) = plan.declared_length {
            if stream.write_all(&chunk).await.is_err() {
                return;
            }
        } else {
            let prefix = format!("{:x}\r\n", chunk.len());
            if stream.write_all(prefix.as_bytes()).await.is_err()
                || stream.write_all(&chunk).await.is_err()
                || stream.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
    }
    responses_started.fetch_add(1, Ordering::SeqCst);

    if plan.complete {
        if plan.declared_length.is_none() {
            let _ = stream.write_all(b"0\r\n\r\n").await;
        }
        let _ = stream.shutdown().await;
    } else if plan.observe_disconnect {
        let disconnected = timeout(Duration::from_secs(2), async {
            let mut byte = [0_u8; 64];
            loop {
                match stream.read(&mut byte).await {
                    Ok(0) | Err(_) => break true,
                    Ok(_) => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        if disconnected {
            disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<CapturedRequest> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
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
    Ok(CapturedRequest {
        method,
        path,
        headers,
        body: Bytes::copy_from_slice(&bytes[header_end..header_end + available]),
    })
}
