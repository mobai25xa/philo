//! Deterministic scripted transport used by contract and client tests.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::{HeaderMap, Method, StatusCode};
use tokio::sync::Notify;
use tokio::time::sleep;

use crate::domain::LocalRequestId;
use crate::error::LlmError;
use crate::provider::{RedirectPolicy, ResolvedEndpoint};

use super::{
    ByteStream, HttpRequest, HttpResponse, RequestLifecycle, Transport, TransportContext,
    TransportFuture, await_with_lifecycle, lifecycle_preflight,
};

/// One delayed body item returned by [`MockTransport`].
pub struct MockBodyItem {
    delay: Duration,
    item: Result<Bytes, LlmError>,
    gate: Option<MockGate>,
}

/// Deterministic one-way gate for fault and race tests.
#[derive(Clone, Default)]
pub struct MockGate {
    inner: Arc<MockGateInner>,
}

#[derive(Default)]
struct MockGateInner {
    open: AtomicBool,
    notify: Notify,
}

impl MockGate {
    /// Creates a closed gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the gate permanently and wakes all current waiters.
    pub fn open(&self) {
        self.inner.open.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    /// Returns whether the gate has been opened.
    pub fn is_open(&self) -> bool {
        self.inner.open.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        while !self.is_open() {
            let notified = self.inner.notify.notified();
            if self.is_open() {
                break;
            }
            notified.await;
        }
    }
}

impl MockBodyItem {
    /// Creates an immediate byte chunk.
    pub fn chunk(bytes: impl Into<Bytes>) -> Self {
        Self {
            delay: Duration::ZERO,
            item: Ok(bytes.into()),
            gate: None,
        }
    }

    /// Creates a delayed byte chunk.
    pub fn delayed_chunk(delay: Duration, bytes: impl Into<Bytes>) -> Self {
        Self {
            delay,
            item: Ok(bytes.into()),
            gate: None,
        }
    }

    /// Creates an immediate body error.
    pub fn error(error: LlmError) -> Self {
        Self {
            delay: Duration::ZERO,
            item: Err(error),
            gate: None,
        }
    }

    /// Creates a delayed body error.
    pub fn delayed_error(delay: Duration, error: LlmError) -> Self {
        Self {
            delay,
            item: Err(error),
            gate: None,
        }
    }

    /// Blocks this body item at a deterministic gate before applying its delay.
    #[must_use]
    pub fn behind_gate(mut self, gate: MockGate) -> Self {
        self.gate = Some(gate);
        self
    }
}

/// Scripted response metadata and body items.
pub struct MockResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<MockBodyItem>,
}

impl MockResponse {
    /// Creates a scripted response.
    pub fn new(status: StatusCode, headers: HeaderMap, body: Vec<MockBodyItem>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }
}

enum MockOutcome {
    Response(MockResponse),
    StartError(LlmError),
}

/// One queued transport start outcome with optional response-header delay.
pub struct MockExchange {
    start_delay: Duration,
    outcome: MockOutcome,
}

impl MockExchange {
    /// Creates an immediate response exchange.
    pub fn response(response: MockResponse) -> Self {
        Self {
            start_delay: Duration::ZERO,
            outcome: MockOutcome::Response(response),
        }
    }

    /// Creates an immediate start error exchange.
    pub fn start_error(error: LlmError) -> Self {
        Self {
            start_delay: Duration::ZERO,
            outcome: MockOutcome::StartError(error),
        }
    }

    /// Delays the start result, including response status and headers.
    #[must_use]
    pub fn with_start_delay(mut self, delay: Duration) -> Self {
        self.start_delay = delay;
        self
    }
}

/// Safe request capture retained by [`MockTransport`].
#[derive(Clone)]
pub struct CapturedRequest {
    method: Method,
    endpoint: ResolvedEndpoint,
    headers: HeaderMap,
    body: Bytes,
    context: TransportContext,
    deadline: Option<tokio::time::Instant>,
    cancelled_at_capture: bool,
    redirect_policy: RedirectPolicy,
}

impl CapturedRequest {
    fn from_request(request: &HttpRequest) -> Self {
        Self {
            method: request.method.clone(),
            endpoint: request.endpoint.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
            context: request.context.clone(),
            deadline: request.lifecycle.deadline(),
            cancelled_at_capture: request.lifecycle.cancellation().is_cancelled(),
            redirect_policy: request.redirect_policy,
        }
    }

    /// Returns captured method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// Returns captured endpoint.
    pub fn endpoint(&self) -> &ResolvedEndpoint {
        &self.endpoint
    }

    /// Returns captured headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns captured body bytes.
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Returns local request identifier.
    pub fn local_request_id(&self) -> &LocalRequestId {
        self.context.local_request_id()
    }

    /// Returns captured absolute deadline.
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.deadline
    }

    /// Returns whether cancellation was already visible when captured.
    pub fn was_cancelled(&self) -> bool {
        self.cancelled_at_capture
    }

    /// Returns captured redirect policy.
    pub fn redirect_policy(&self) -> RedirectPolicy {
        self.redirect_policy
    }
}

impl fmt::Debug for CapturedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<_> = self.headers.keys().map(http::HeaderName::as_str).collect();
        f.debug_struct("CapturedRequest")
            .field("method", &self.method)
            .field("origin", self.endpoint.origin())
            .field("header_names", &header_names)
            .field("body_len", &self.body.len())
            .field("context", &self.context)
            .field("has_deadline", &self.deadline.is_some())
            .field("cancelled_at_capture", &self.cancelled_at_capture)
            .field("redirect_policy", &self.redirect_policy)
            .finish()
    }
}

#[derive(Default)]
struct MockInner {
    exchanges: Mutex<VecDeque<MockExchange>>,
    captured: Mutex<Vec<CapturedRequest>>,
    early_body_drops: AtomicUsize,
    body_cancellations: AtomicUsize,
    body_polls: AtomicUsize,
}

/// Concurrent scripted transport with request capture and lifecycle observation.
#[derive(Clone, Default)]
pub struct MockTransport {
    inner: Arc<MockInner>,
}

impl MockTransport {
    /// Creates a mock from queued exchanges.
    pub fn scripted(exchanges: impl IntoIterator<Item = MockExchange>) -> Self {
        Self {
            inner: Arc::new(MockInner {
                exchanges: Mutex::new(exchanges.into_iter().collect()),
                ..MockInner::default()
            }),
        }
    }

    /// Appends one exchange to the queue.
    pub fn push(&self, exchange: MockExchange) {
        lock(&self.inner.exchanges).push_back(exchange);
    }

    /// Returns captured requests in transport arrival order.
    pub fn captured_requests(&self) -> Vec<CapturedRequest> {
        lock(&self.inner.captured).clone()
    }

    /// Drains captured request fixtures so long-running harnesses retain bounded memory.
    pub fn drain_captured_requests(&self) -> Vec<CapturedRequest> {
        lock(&self.inner.captured).drain(..).collect()
    }

    /// Returns the number of unconsumed exchanges.
    pub fn remaining_expectations(&self) -> usize {
        lock(&self.inner.exchanges).len()
    }

    /// Panics if scripted exchanges remain unconsumed.
    pub fn assert_consumed(&self) {
        assert_eq!(
            self.remaining_expectations(),
            0,
            "unconsumed mock exchanges"
        );
    }

    /// Returns body streams dropped before their scripted terminal state.
    pub fn early_body_drop_count(&self) -> usize {
        self.inner.early_body_drops.load(Ordering::SeqCst)
    }

    /// Returns body streams that observed explicit cancellation.
    pub fn body_cancellation_count(&self) -> usize {
        self.inner.body_cancellations.load(Ordering::SeqCst)
    }

    /// Returns the number of scripted body items requested by consumers.
    pub fn body_poll_count(&self) -> usize {
        self.inner.body_polls.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for MockTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockTransport")
            .field("captured_count", &lock(&self.inner.captured).len())
            .field("remaining_expectations", &self.remaining_expectations())
            .field("early_body_drops", &self.early_body_drop_count())
            .field("body_cancellations", &self.body_cancellation_count())
            .field("body_polls", &self.body_poll_count())
            .finish()
    }
}

impl Transport for MockTransport {
    fn execute(&self, request: HttpRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            lifecycle_preflight(&request.lifecycle)?;
            let exchange = lock(&self.inner.exchanges)
                .pop_front()
                .ok_or_else(|| LlmError::Configuration("mock transport script exhausted".into()))?;
            lock(&self.inner.captured).push(CapturedRequest::from_request(&request));

            await_with_lifecycle(&request.lifecycle, sleep(exchange.start_delay)).await?;
            match exchange.outcome {
                MockOutcome::StartError(error) => Err(error),
                MockOutcome::Response(response) => {
                    let body =
                        mock_body_stream(response.body, request.lifecycle, Arc::clone(&self.inner));
                    Ok(HttpResponse::new(response.status, response.headers, body))
                }
            }
        })
    }
}

struct MockBodyGuard {
    inner: Arc<MockInner>,
    completed: bool,
}

impl MockBodyGuard {
    fn complete(&mut self) {
        self.completed = true;
    }

    fn cancel(&mut self) {
        self.inner.body_cancellations.fetch_add(1, Ordering::SeqCst);
        self.completed = true;
    }
}

impl Drop for MockBodyGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.inner.early_body_drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct MockBodyState {
    items: VecDeque<MockBodyItem>,
    lifecycle: RequestLifecycle,
    guard: MockBodyGuard,
    terminated: bool,
}

fn mock_body_stream(
    items: Vec<MockBodyItem>,
    lifecycle: RequestLifecycle,
    inner: Arc<MockInner>,
) -> ByteStream {
    let state = MockBodyState {
        items: items.into(),
        lifecycle,
        guard: MockBodyGuard {
            inner,
            completed: false,
        },
        terminated: false,
    };
    Box::pin(stream::unfold(state, |mut state| async move {
        if state.terminated {
            state.guard.complete();
            return None;
        }
        let Some(item) = state.items.pop_front() else {
            state.guard.complete();
            return None;
        };
        state.guard.inner.body_polls.fetch_add(1, Ordering::SeqCst);
        let MockBodyItem { delay, item, gate } = item;
        let readiness = async move {
            if let Some(gate) = gate {
                gate.wait().await;
            }
            sleep(delay).await;
        };
        match await_with_lifecycle(&state.lifecycle, readiness).await {
            Ok(()) => {
                if item.is_err() {
                    state.terminated = true;
                    state.guard.complete();
                }
                Some((item, state))
            }
            Err(error) => {
                state.terminated = true;
                if matches!(error, LlmError::Cancelled) {
                    state.guard.cancel();
                } else {
                    state.guard.complete();
                }
                Some((Err(error), state))
            }
        }
    }))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
