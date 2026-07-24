//! Production HTTP transport backed by a shared `reqwest::Client`.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures_core::Stream;
use futures_util::{StreamExt as _, stream};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use thiserror::Error;

use crate::error::{ErrorStage, LlmError, RetriableHint, TimeoutError, TransportError};
use crate::provider::{EndpointNetworkPolicy, Origin, RedirectPolicy};

use super::{
    ByteStream, HttpRequest, HttpResponse, RequestLifecycle, Transport, TransportFuture,
    await_with_lifecycle,
};

const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Error)]
#[error("reqwest transport failure: {kind}")]
struct SafeReqwestSource {
    kind: &'static str,
}

#[derive(Debug, Error)]
#[error("secure DNS resolution failure")]
struct SafeDnsSource;

#[derive(Clone, Copy, Debug)]
struct SecureDnsResolver;

impl Resolve for SecureDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|_| Box::new(SafeDnsSource) as Box<dyn std::error::Error + Send + Sync>)?
                .collect::<Vec<SocketAddr>>();
            let policy = if host.eq_ignore_ascii_case("localhost") {
                EndpointNetworkPolicy::test_loopback()
            } else {
                EndpointNetworkPolicy::public_https()
            };
            policy
                .validate_resolved_addresses(addresses.iter().map(SocketAddr::ip))
                .map_err(|_| Box::new(SafeDnsSource) as Box<dyn std::error::Error + Send + Sync>)?;
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

/// Shared production HTTP transport. Public methods expose no `reqwest` types.
#[derive(Clone)]
pub struct ReqwestTransport {
    redirects_disabled: reqwest::Client,
    same_origin_redirects: reqwest::Client,
}

impl ReqwestTransport {
    /// Builds shared clients with no default headers and rustls when enabled.
    pub fn new() -> Result<Self, LlmError> {
        Ok(Self {
            redirects_disabled: build_client(Policy::none())?,
            same_origin_redirects: build_client(same_origin_policy())?,
        })
    }
}

impl fmt::Debug for ReqwestTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestTransport")
            .field("shared_clients", &2)
            .field("max_same_origin_redirects", &MAX_REDIRECTS)
            .finish()
    }
}

impl Transport for ReqwestTransport {
    fn execute(&self, request: HttpRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            let client = match request.redirect_policy {
                RedirectPolicy::Disabled => &self.redirects_disabled,
                RedirectPolicy::SameOrigin => &self.same_origin_redirects,
            };
            let lifecycle = request.lifecycle;
            let send = client
                .request(request.method, request.endpoint.url().clone())
                .headers(request.headers)
                .body(request.body)
                .send();
            let response = await_with_lifecycle(&lifecycle, send)
                .await?
                .map_err(|error| map_start_error(&error))?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = reqwest_body_stream(response, lifecycle);
            Ok(HttpResponse::new(status, headers, body))
        })
    }
}

fn build_client(redirect: Policy) -> Result<reqwest::Client, LlmError> {
    let builder = reqwest::Client::builder()
        .redirect(redirect)
        .dns_resolver(Arc::new(SecureDnsResolver));
    #[cfg(feature = "rustls-tls")]
    let builder = builder.use_rustls_tls();
    builder.build().map_err(|_| {
        TransportError::with_source(
            ErrorStage::Configuration,
            RetriableHint::No,
            SafeReqwestSource {
                kind: "client-build",
            },
        )
        .into()
    })
}

fn same_origin_policy() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error(io::Error::other("redirect limit exceeded"));
        }
        let Some(initial) = attempt.previous().first() else {
            return attempt.error(io::Error::other("redirect has no initial URL"));
        };
        let next_url = attempt.url();
        let safe_shape = next_url.username().is_empty()
            && next_url.password().is_none()
            && next_url.query().is_none()
            && next_url.fragment().is_none();
        match (Origin::from_url(initial), Origin::from_url(next_url)) {
            (Ok(initial), Ok(next))
                if safe_shape
                    && initial == next
                    && !(initial.scheme() == "https" && next.scheme() != "https") =>
            {
                attempt.follow()
            }
            _ => attempt.error(io::Error::other("cross-origin redirect rejected")),
        }
    })
}

type ReqwestByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;

struct ReqwestBodyState {
    inner: ReqwestByteStream,
    lifecycle: RequestLifecycle,
    terminated: bool,
}

fn reqwest_body_stream(response: reqwest::Response, lifecycle: RequestLifecycle) -> ByteStream {
    let state = ReqwestBodyState {
        inner: Box::pin(response.bytes_stream()),
        lifecycle,
        terminated: false,
    };
    Box::pin(stream::unfold(state, |mut state| async move {
        if state.terminated {
            return None;
        }
        let lifecycle = state.lifecycle.clone();
        match await_with_lifecycle(&lifecycle, state.inner.next()).await {
            Err(error) => {
                state.terminated = true;
                Some((Err(error), state))
            }
            Ok(None) => None,
            Ok(Some(Ok(bytes))) => Some((Ok(bytes), state)),
            Ok(Some(Err(error))) => {
                state.terminated = true;
                Some((Err(map_body_error(&error)), state))
            }
        }
    }))
}

fn map_start_error(error: &reqwest::Error) -> LlmError {
    if error.is_timeout() {
        return TimeoutError::new(ErrorStage::Timeout).into();
    }
    let (stage, hint, kind) = if error.is_connect() {
        let stage = if is_tls_error(error) {
            ErrorStage::Tls
        } else {
            ErrorStage::Connect
        };
        let kind = if stage == ErrorStage::Tls {
            "tls"
        } else {
            "connect"
        };
        (stage, RetriableHint::Maybe, kind)
    } else if error.is_body() {
        (ErrorStage::Body, RetriableHint::Maybe, "body")
    } else {
        (ErrorStage::Http, RetriableHint::No, "http")
    };
    TransportError::with_source(stage, hint, SafeReqwestSource { kind }).into()
}

fn map_body_error(error: &reqwest::Error) -> LlmError {
    if error.is_timeout() {
        TimeoutError::new(ErrorStage::Timeout).into()
    } else {
        TransportError::with_source(
            ErrorStage::Body,
            RetriableHint::Maybe,
            SafeReqwestSource { kind: "body" },
        )
        .into()
    }
}

#[cfg(feature = "rustls-tls")]
fn is_tls_error(error: &reqwest::Error) -> bool {
    contains_tls_error(error, 0)
}

#[cfg(feature = "rustls-tls")]
fn contains_tls_error(error: &(dyn std::error::Error + 'static), depth: u8) -> bool {
    if depth > 12 || error.is::<rustls::Error>() {
        return error.is::<rustls::Error>();
    }
    if let Some(error) = error.downcast_ref::<io::Error>()
        && let Some(inner) = error.get_ref()
        && contains_tls_error(inner, depth + 1)
    {
        return true;
    }
    error
        .source()
        .is_some_and(|source| contains_tls_error(source, depth + 1))
}

#[cfg(not(feature = "rustls-tls"))]
fn is_tls_error(_error: &reqwest::Error) -> bool {
    false
}
