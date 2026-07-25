//! Production HTTP transport backed by a shared `reqwest::Client`.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use futures_util::{StreamExt as _, stream};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use thiserror::Error;

#[cfg(feature = "rustls-tls")]
use super::MinimumTlsVersion;
use super::{
    ByteStream, HttpRequest, HttpResponse, HttpVersionPolicy, IpPreference, NetworkPolicy,
    RequestLifecycle, Transport, TransportFuture, await_with_lifecycle,
};
use crate::error::{
    ErrorStage, LlmError, RetriableHint, TimeoutError, TimeoutStage, TransportError,
};
use crate::provider::{EndpointNetworkPolicy, Origin, RedirectPolicy};

const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Error)]
#[error("reqwest transport failure: {kind}")]
struct SafeReqwestSource {
    kind: &'static str,
}

#[derive(Debug, Error)]
#[error("secure DNS resolution failure")]
struct SafeDnsSource;

#[derive(Clone, Debug)]
struct SecureDnsResolver {
    timeout: Duration,
    preference: IpPreference,
    allowed_private_hosts: Arc<HashSet<String>>,
}

impl Resolve for SecureDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        let timeout = self.timeout;
        let preference = self.preference;
        let allowed_private_hosts = Arc::clone(&self.allowed_private_hosts);
        Box::pin(async move {
            let addresses =
                tokio::time::timeout(timeout, tokio::net::lookup_host((host.as_str(), 0)))
                    .await
                    .map_err(|_| {
                        Box::new(SafeDnsSource) as Box<dyn std::error::Error + Send + Sync>
                    })?
                    .map_err(|_| {
                        Box::new(SafeDnsSource) as Box<dyn std::error::Error + Send + Sync>
                    })?
                    .collect::<Vec<SocketAddr>>();
            let allow_private =
                host.eq_ignore_ascii_case("localhost") || allowed_private_hosts.contains(&host);
            let policy = if allow_private {
                EndpointNetworkPolicy::test_loopback()
            } else {
                EndpointNetworkPolicy::public_https()
            };
            if !allowed_private_hosts.contains(&host) {
                policy
                    .validate_resolved_addresses(addresses.iter().map(SocketAddr::ip))
                    .map_err(|_| {
                        Box::new(SafeDnsSource) as Box<dyn std::error::Error + Send + Sync>
                    })?;
            }
            let mut addresses = addresses;
            match preference {
                IpPreference::System => {}
                IpPreference::Ipv4First => addresses.sort_by_key(SocketAddr::is_ipv6),
                IpPreference::Ipv6First => addresses.sort_by_key(SocketAddr::is_ipv4),
            }
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
    policy: NetworkPolicy,
}

impl ReqwestTransport {
    /// Builds shared clients with no default headers and rustls when enabled.
    pub fn new() -> Result<Self, LlmError> {
        Self::with_policy(NetworkPolicy::secure_defaults())
    }

    /// Builds one bounded shared client pair from an immutable network policy.
    pub fn with_policy(policy: NetworkPolicy) -> Result<Self, LlmError> {
        Ok(Self {
            redirects_disabled: build_client(Policy::none(), &policy)?,
            same_origin_redirects: build_client(same_origin_policy(), &policy)?,
            policy,
        })
    }

    /// Returns the implementation-neutral policy frozen into this transport.
    #[must_use]
    pub const fn network_policy(&self) -> &NetworkPolicy {
        &self.policy
    }
}

impl fmt::Debug for ReqwestTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestTransport")
            .field("shared_clients", &2)
            .field("max_same_origin_redirects", &MAX_REDIRECTS)
            .field("network_policy", &self.policy)
            .finish_non_exhaustive()
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

fn build_client(redirect: Policy, policy: &NetworkPolicy) -> Result<reqwest::Client, LlmError> {
    let resolved_proxy = policy.proxy().resolve()?;
    let allowed_private_hosts = resolved_proxy
        .as_ref()
        .and_then(|proxy| proxy.endpoint.host_str())
        .map(|host| HashSet::from([host.trim_end_matches('.').to_ascii_lowercase()]))
        .unwrap_or_default();
    let resolver = SecureDnsResolver {
        timeout: policy.dns().timeout(),
        preference: policy.dns().ip_preference(),
        allowed_private_hosts: Arc::new(allowed_private_hosts),
    };
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(redirect)
        .no_proxy()
        .pool_idle_timeout(policy.pool().idle_timeout())
        .pool_max_idle_per_host(policy.pool().max_idle_per_host())
        .tcp_keepalive(policy.pool().tcp_keepalive())
        .dns_resolver(Arc::new(resolver));
    if policy.pool().http_version() == HttpVersionPolicy::Http1Only {
        builder = builder.http1_only();
    } else {
        builder = builder
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .http2_keep_alive_while_idle(false);
    }
    if let Some(proxy) = resolved_proxy {
        let mut reqwest_proxy = reqwest::Proxy::all(proxy.endpoint.as_str())
            .map_err(|_| client_build_error("proxy-config"))?;
        if let Some(credentials) = proxy.credentials {
            let (username, password) = credentials.parts();
            reqwest_proxy = reqwest_proxy.basic_auth(username, password);
        }
        if !proxy.no_proxy.is_empty() {
            reqwest_proxy =
                reqwest_proxy.no_proxy(reqwest::NoProxy::from_string(&proxy.no_proxy.joined()));
        }
        builder = builder.proxy(reqwest_proxy);
    }
    let builder = apply_tls_policy(builder, policy)?;
    builder
        .build()
        .map_err(|_| client_build_error("client-build"))
}

#[cfg(feature = "rustls-tls")]
fn apply_tls_policy(
    mut builder: reqwest::ClientBuilder,
    policy: &NetworkPolicy,
) -> Result<reqwest::ClientBuilder, LlmError> {
    builder = builder.min_tls_version(match policy.tls().minimum_version() {
        MinimumTlsVersion::Tls12 => reqwest::tls::Version::TLS_1_2,
        MinimumTlsVersion::Tls13 => reqwest::tls::Version::TLS_1_3,
    });
    for pem in policy.tls().custom_roots() {
        let certificates = reqwest::Certificate::from_pem_bundle(pem)
            .map_err(|_| client_build_error("custom-ca"))?;
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    if let Some(identity) = policy.tls().client_identity() {
        let identity = reqwest::Identity::from_pem(identity)
            .map_err(|_| client_build_error("client-identity"))?;
        builder = builder.identity(identity);
    }
    Ok(builder.use_rustls_tls())
}

#[cfg(not(feature = "rustls-tls"))]
fn apply_tls_policy(
    builder: reqwest::ClientBuilder,
    policy: &NetworkPolicy,
) -> Result<reqwest::ClientBuilder, LlmError> {
    if !policy.tls().custom_roots().is_empty() || policy.tls().client_identity().is_some() {
        return Err(client_build_error("tls-feature-disabled"));
    }
    Ok(builder)
}

fn client_build_error(kind: &'static str) -> LlmError {
    TransportError::with_source(
        ErrorStage::Configuration,
        RetriableHint::No,
        SafeReqwestSource { kind },
    )
    .into()
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
        let stage = if error.is_connect() {
            TimeoutStage::Connect
        } else {
            TimeoutStage::ResponseHeader
        };
        return TimeoutError::new(stage).into();
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
        TimeoutError::new(TimeoutStage::UnknownTransport).into()
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
