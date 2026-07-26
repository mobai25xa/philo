//! HTTP transport contracts that expose status, headers, and byte streams.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::StreamExt as _;
use http::{HeaderMap, Method, StatusCode};
use tokio::time::Instant;

use crate::domain::LocalRequestId;
use crate::error::{BodySummary, LlmError, TimeoutError, TimeoutStage};
use crate::provider::{RedirectPolicy, ResolvedEndpoint};

#[doc(hidden)]
pub mod mock;
mod network;
mod reqwest;
mod sse;

pub use network::{
    ConnectionPoolPolicy, DnsPolicy, ExplicitProxy, HttpVersionPolicy, IpPreference,
    MinimumTlsVersion, NetworkPolicy, NoProxyList, ProxyCredentials, ProxyPolicy, TlsPolicy,
};
pub use reqwest::ReqwestTransport;
pub use sse::{SseConfig, SseDecoder, SseError, SseEvent, SseLimit};

/// A boxed, backpressure-aware response byte stream.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, LlmError>> + Send + 'static>>;

/// A boxed transport start future.
pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, LlmError>> + Send + 'a>>;

/// SDK-owned cancellation handle shared across request lifecycle stages.
#[derive(Clone, Default)]
pub struct CancellationToken(tokio_util::sync::CancellationToken);

impl CancellationToken {
    /// Creates an independent cancellation token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels this token and every waiter using a clone of it.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    pub(crate) async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

impl fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancellationToken")
            .field("is_cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Debug)]
struct DeadlineBudget {
    started: Instant,
    deadline: Option<Instant>,
}

impl DeadlineBudget {
    fn new(deadline: Option<Instant>) -> Self {
        Self {
            started: Instant::now(),
            deadline,
        }
    }
}

/// Shared absolute deadline and cancellation state that never restarts across attempts.
#[derive(Clone, Debug)]
pub struct RequestLifecycle {
    budget: Arc<DeadlineBudget>,
    cancellation: CancellationToken,
}

impl Default for RequestLifecycle {
    fn default() -> Self {
        Self::new(CancellationToken::new())
    }
}

impl RequestLifecycle {
    /// Creates lifecycle state without a deadline.
    pub fn new(cancellation: CancellationToken) -> Self {
        Self {
            budget: Arc::new(DeadlineBudget::new(None)),
            cancellation,
        }
    }

    /// Sets the absolute overall deadline.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.budget = Arc::new(DeadlineBudget {
            started: self.budget.started,
            deadline: Some(deadline),
        });
        self
    }

    /// Returns the absolute deadline, if one was configured.
    pub fn deadline(&self) -> Option<Instant> {
        self.budget.deadline
    }

    /// Returns the shared cancellation handle.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub(crate) fn started_at(&self) -> Instant {
        self.budget.started
    }

    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.budget
            .deadline
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

/// Non-sensitive identifiers available to transport diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportContext {
    local_request_id: LocalRequestId,
}

impl TransportContext {
    /// Creates diagnostic context for one local SDK request.
    pub fn new(local_request_id: LocalRequestId) -> Self {
        Self { local_request_id }
    }

    /// Returns the local request identifier.
    pub fn local_request_id(&self) -> &LocalRequestId {
        &self.local_request_id
    }
}

/// Fully resolved HTTP request accepted by a [`Transport`].
pub struct HttpRequest {
    pub(crate) method: Method,
    pub(crate) endpoint: ResolvedEndpoint,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
    pub(crate) lifecycle: RequestLifecycle,
    pub(crate) context: TransportContext,
    pub(crate) redirect_policy: RedirectPolicy,
}

impl HttpRequest {
    /// Creates a request with no deadline, a fresh cancellation token, and redirects disabled.
    pub fn new(
        method: Method,
        endpoint: ResolvedEndpoint,
        headers: HeaderMap,
        body: Bytes,
        context: TransportContext,
    ) -> Self {
        Self {
            method,
            endpoint,
            headers,
            body,
            lifecycle: RequestLifecycle::default(),
            context,
            redirect_policy: RedirectPolicy::Disabled,
        }
    }

    /// Replaces the lifecycle state.
    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: RequestLifecycle) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    /// Applies the validated redirect policy snapshot.
    #[must_use]
    pub fn with_redirect_policy(mut self, policy: RedirectPolicy) -> Self {
        self.redirect_policy = policy;
        self
    }

    /// Returns the HTTP method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// Returns the validated endpoint.
    pub fn endpoint(&self) -> &ResolvedEndpoint {
        &self.endpoint
    }

    /// Returns request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns request body bytes.
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Returns lifecycle state.
    pub fn lifecycle(&self) -> &RequestLifecycle {
        &self.lifecycle
    }

    /// Returns non-sensitive diagnostic context.
    pub fn context(&self) -> &TransportContext {
        &self.context
    }

    /// Returns redirect policy.
    pub fn redirect_policy(&self) -> RedirectPolicy {
        self.redirect_policy
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<_> = self.headers.keys().map(http::HeaderName::as_str).collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("origin", self.endpoint.origin())
            .field("header_names", &header_names)
            .field("body_len", &self.body.len())
            .field("lifecycle", &self.lifecycle)
            .field("context", &self.context)
            .field("redirect_policy", &self.redirect_policy)
            .finish()
    }
}

/// HTTP response metadata and streaming body.
pub struct HttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: ByteStream,
}

impl HttpResponse {
    /// Creates a response from SDK HTTP types.
    pub fn new(status: StatusCode, headers: HeaderMap, body: ByteStream) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns status before body consumption.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns headers before body consumption.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Consumes the response and returns its byte stream.
    pub fn into_body(self) -> ByteStream {
        self.body
    }

    /// Consumes the response into status, headers, and body.
    pub fn into_parts(self) -> (StatusCode, HeaderMap, ByteStream) {
        (self.status, self.headers, self.body)
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<_> = self.headers.keys().map(http::HeaderName::as_str).collect();
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("header_names", &header_names)
            .field("body", &"<byte stream>")
            .finish()
    }
}

/// HTTP transport that performs no protocol-level parsing.
pub trait Transport: Send + Sync {
    /// Starts one request. Body failures are yielded by the returned response stream.
    fn execute(&self, request: HttpRequest) -> TransportFuture<'_>;
}

/// Bounded body prefix collected without reading beyond the configured limit.
#[derive(Clone, Eq, PartialEq)]
pub struct LimitedBody {
    bytes: Bytes,
    truncated: bool,
}

impl LimitedBody {
    #[cfg(test)]
    pub(crate) fn from_test_parts(bytes: Bytes, truncated: bool) -> Self {
        Self { bytes, truncated }
    }

    /// Returns the retained body prefix.
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// Returns whether unread body bytes were discarded.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Builds a redacted, UTF-8-safe diagnostic summary.
    pub fn summary(&self) -> BodySummary {
        BodySummary::from_prefix(&self.bytes, self.truncated)
    }
}

impl fmt::Debug for LimitedBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LimitedBody")
            .field("len", &self.bytes.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// Reads at most `limit` bytes and then drops the remaining stream.
pub async fn read_body_limited(
    mut body: ByteStream,
    limit: usize,
) -> Result<LimitedBody, LlmError> {
    let mut bytes = BytesMut::with_capacity(limit.min(8 * 1024));
    let mut truncated = false;
    while let Some(item) = body.next().await {
        let chunk = item?;
        let remaining = limit.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == limit {
            while let Some(item) = body.next().await {
                match item {
                    Ok(chunk) if chunk.is_empty() => {}
                    Ok(_) => {
                        truncated = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            break;
        }
    }
    Ok(LimitedBody {
        bytes: bytes.freeze(),
        truncated,
    })
}

pub(crate) fn lifecycle_preflight(lifecycle: &RequestLifecycle) -> Result<(), LlmError> {
    if lifecycle.cancellation.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    if lifecycle
        .budget
        .deadline
        .is_some_and(|deadline| deadline <= Instant::now())
    {
        return Err(TimeoutError::new(TimeoutStage::Overall)
            .with_context(true, lifecycle.started_at().elapsed(), None, None, false)
            .into());
    }
    Ok(())
}

pub(crate) async fn await_with_lifecycle<F, T>(
    lifecycle: &RequestLifecycle,
    future: F,
) -> Result<T, LlmError>
where
    F: Future<Output = T>,
{
    await_with_stage(lifecycle, TimeoutStage::Overall, None, None, false, future).await
}

pub(crate) async fn await_with_stage<F, T>(
    lifecycle: &RequestLifecycle,
    stage: TimeoutStage,
    stage_limit: Option<Duration>,
    attempt_number: Option<u32>,
    domain_event_delivered: bool,
    future: F,
) -> Result<T, LlmError>
where
    F: Future<Output = T>,
{
    lifecycle_preflight(lifecycle)?;
    let now = Instant::now();
    let stage_deadline = stage_limit.and_then(|limit| now.checked_add(limit));
    let (effective_deadline, overall_limited) = match (lifecycle.deadline(), stage_deadline) {
        (Some(overall), Some(stage)) if overall <= stage => (Some(overall), true),
        (_, Some(stage)) => (Some(stage), false),
        (Some(overall), None) => (Some(overall), true),
        (None, None) => (None, false),
    };
    if let Some(deadline) = effective_deadline {
        tokio::select! {
            biased;
            () = lifecycle.cancellation.cancelled() => Err(LlmError::Cancelled),
            () = tokio::time::sleep_until(deadline) => {
                Err(TimeoutError::new(stage)
                    .with_context(
                        overall_limited,
                        lifecycle.started_at().elapsed(),
                        stage_limit,
                        attempt_number,
                        domain_event_delivered,
                    )
                    .into())
            }
            output = future => Ok(output),
        }
    } else {
        tokio::select! {
            biased;
            () = lifecycle.cancellation.cancelled() => Err(LlmError::Cancelled),
            output = future => Ok(output),
        }
    }
}

/// Waits for stream progress while allowing the active body stream to observe cancellation first.
pub(crate) async fn await_stream_with_stage<F, T>(
    lifecycle: &RequestLifecycle,
    stage: TimeoutStage,
    stage_limit: Duration,
    attempt_number: u32,
    domain_event_delivered: bool,
    future: F,
) -> Result<T, LlmError>
where
    F: Future<Output = T>,
{
    if lifecycle
        .deadline()
        .is_some_and(|deadline| deadline <= Instant::now())
    {
        return Err(TimeoutError::new(TimeoutStage::Overall)
            .with_context(
                true,
                lifecycle.started_at().elapsed(),
                Some(stage_limit),
                Some(attempt_number),
                domain_event_delivered,
            )
            .into());
    }
    let now = Instant::now();
    let stage_deadline = now.checked_add(stage_limit);
    let (effective_deadline, overall_limited) = match (lifecycle.deadline(), stage_deadline) {
        (Some(overall), Some(stage)) if overall <= stage => (Some(overall), true),
        (_, Some(stage)) => (Some(stage), false),
        (Some(overall), None) => (Some(overall), true),
        (None, None) => (None, false),
    };
    if let Some(deadline) = effective_deadline {
        tokio::select! {
            biased;
            output = future => Ok(output),
            () = lifecycle.cancellation.cancelled() => Err(LlmError::Cancelled),
            () = tokio::time::sleep_until(deadline) => {
                Err(TimeoutError::new(stage)
                    .with_context(
                        overall_limited,
                        lifecycle.started_at().elapsed(),
                        Some(stage_limit),
                        Some(attempt_number),
                        domain_event_delivered,
                    )
                    .into())
            }
        }
    } else {
        tokio::select! {
            biased;
            output = future => Ok(output),
            () = lifecycle.cancellation.cancelled() => Err(LlmError::Cancelled),
        }
    }
}
