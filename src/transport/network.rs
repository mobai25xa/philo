//! Immutable, implementation-neutral network policy for the shared HTTP transport.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretSlice, SecretString};
use url::Url;

use crate::error::{LlmError, ValidationError, ValidationReason};

const MAX_PROXY_URL_BYTES: usize = 2_048;
const MAX_PROXY_CREDENTIAL_BYTES: usize = 512;
const MAX_NO_PROXY_ENTRIES: usize = 64;
const MAX_NO_PROXY_BYTES: usize = 4_096;
const MAX_CUSTOM_ROOTS: usize = 8;
const MAX_CUSTOM_ROOT_BYTES: usize = 256 * 1024;
const MAX_CLIENT_IDENTITY_BYTES: usize = 512 * 1024;
const MAX_DNS_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_POOL_IDLE_TIMEOUT: Duration = Duration::from_mins(10);
const MAX_IDLE_PER_HOST: usize = 64;

/// Address-family ordering applied after secure DNS filtering.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum IpPreference {
    /// Preserve resolver order.
    #[default]
    System,
    /// Prefer IPv4 while retaining valid IPv6 fallbacks.
    Ipv4First,
    /// Prefer IPv6 while retaining valid IPv4 fallbacks.
    Ipv6First,
}

/// HTTP protocol negotiation policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum HttpVersionPolicy {
    /// Negotiate HTTP/1.1 or HTTP/2 securely.
    #[default]
    Negotiate,
    /// Restrict connections to HTTP/1.1.
    Http1Only,
}

/// Minimum accepted TLS protocol version.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum MinimumTlsVersion {
    /// Require TLS 1.2 or newer.
    #[default]
    Tls12,
    /// Require TLS 1.3.
    Tls13,
}

/// Bounded, normalized `NO_PROXY` entries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoProxyList {
    entries: Vec<String>,
}

impl NoProxyList {
    /// Validates a deterministic list of domains, IPs, CIDRs, or the exact `*` entry.
    ///
    /// # Errors
    ///
    /// Returns a validation error for excessive or structurally unsafe entries.
    pub fn new<I, S>(entries: I) -> Result<Self, ValidationError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let entries = entries.into_iter().map(Into::into).collect::<Vec<_>>();
        if entries.len() > MAX_NO_PROXY_ENTRIES {
            return Err(validation(
                "network.proxy.no_proxy",
                ValidationReason::OutOfRange,
                "NO_PROXY entry count exceeds the SDK limit",
            ));
        }
        let total = entries.iter().map(String::len).sum::<usize>();
        if total > MAX_NO_PROXY_BYTES {
            return Err(validation(
                "network.proxy.no_proxy",
                ValidationReason::OutOfRange,
                "NO_PROXY bytes exceed the SDK limit",
            ));
        }
        if entries.iter().any(|entry| {
            entry.is_empty()
                || entry.trim() != entry
                || entry.contains(['\r', '\n', '\0'])
                || entry.contains(',')
        }) {
            return Err(validation(
                "network.proxy.no_proxy",
                ValidationReason::InvalidIdentifier,
                "NO_PROXY contains an invalid entry",
            ));
        }
        Ok(Self { entries })
    }

    /// Returns the number of bounded entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no entries are configured.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn joined(&self) -> String {
        self.entries.join(",")
    }
}

/// Redacted Basic credentials used only for proxy authentication.
#[derive(Clone)]
pub struct ProxyCredentials {
    username: SecretString,
    password: SecretString,
}

impl ProxyCredentials {
    /// Creates bounded proxy credentials.
    ///
    /// # Errors
    ///
    /// Returns a validation error for empty, oversized, or control-character values.
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let username = username.into();
        let password = password.into();
        if username.is_empty() || password.is_empty() {
            return Err(validation(
                "network.proxy.credentials",
                ValidationReason::Empty,
                "proxy credentials must be non-empty",
            ));
        }
        if username.len().saturating_add(password.len()) > MAX_PROXY_CREDENTIAL_BYTES {
            return Err(validation(
                "network.proxy.credentials",
                ValidationReason::OutOfRange,
                "proxy credentials exceed the SDK byte limit",
            ));
        }
        if username.contains(['\r', '\n', '\0']) || password.contains(['\r', '\n', '\0']) {
            return Err(validation(
                "network.proxy.credentials",
                ValidationReason::InvalidHeader,
                "proxy credentials contain control characters",
            ));
        }
        Ok(Self {
            username: SecretString::from(username),
            password: SecretString::from(password),
        })
    }

    pub(crate) fn parts(&self) -> (&str, &str) {
        (self.username.expose_secret(), self.password.expose_secret())
    }
}

impl fmt::Debug for ProxyCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProxyCredentials([REDACTED])")
    }
}

/// Explicit proxy endpoint, authentication, and bypass list.
#[derive(Clone)]
pub struct ExplicitProxy {
    endpoint: Url,
    credentials: Option<ProxyCredentials>,
    no_proxy: NoProxyList,
}

impl ExplicitProxy {
    /// Validates an explicit HTTP(S) proxy URL without URL-embedded credentials or query data.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the endpoint shape is unsafe.
    pub fn new(endpoint: impl AsRef<str>) -> Result<Self, ValidationError> {
        let endpoint = endpoint.as_ref();
        if endpoint.len() > MAX_PROXY_URL_BYTES {
            return Err(validation(
                "network.proxy.endpoint",
                ValidationReason::OutOfRange,
                "proxy endpoint exceeds the SDK byte limit",
            ));
        }
        let endpoint = Url::parse(endpoint).map_err(|_| {
            validation(
                "network.proxy.endpoint",
                ValidationReason::InvalidIdentifier,
                "proxy endpoint is not a valid absolute URL",
            )
        })?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || (endpoint.path() != "/" && !endpoint.path().is_empty())
        {
            return Err(validation(
                "network.proxy.endpoint",
                ValidationReason::InvalidIdentifier,
                "proxy endpoint must be an origin without credentials, path, query, or fragment",
            ));
        }
        Ok(Self {
            endpoint,
            credentials: None,
            no_proxy: NoProxyList::default(),
        })
    }

    /// Attaches redacted proxy-only credentials.
    #[must_use]
    pub fn with_credentials(mut self, credentials: ProxyCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Replaces the bounded explicit bypass list.
    #[must_use]
    pub fn with_no_proxy(mut self, no_proxy: NoProxyList) -> Self {
        self.no_proxy = no_proxy;
        self
    }

    pub(crate) const fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub(crate) const fn credentials(&self) -> Option<&ProxyCredentials> {
        self.credentials.as_ref()
    }

    pub(crate) const fn no_proxy(&self) -> &NoProxyList {
        &self.no_proxy
    }
}

impl fmt::Debug for ExplicitProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplicitProxy")
            .field("scheme", &self.endpoint.scheme())
            .field("host", &self.endpoint.host_str())
            .field("port", &self.endpoint.port_or_known_default())
            .field("has_credentials", &self.credentials.is_some())
            .field("no_proxy_entries", &self.no_proxy.len())
            .finish()
    }
}

/// Deterministic proxy source policy.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub enum ProxyPolicy {
    /// Do not read proxy environment variables and connect directly.
    #[default]
    Disabled,
    /// Read bounded `HTTPS_PROXY`/`ALL_PROXY` and `NO_PROXY` once when building the client.
    Environment,
    /// Use this explicit configuration instead of environment variables.
    Explicit(ExplicitProxy),
}

/// TLS roots, client identity, and minimum-version policy.
#[derive(Clone)]
pub struct TlsPolicy {
    minimum_version: MinimumTlsVersion,
    custom_roots: Vec<Arc<[u8]>>,
    client_identity: Option<Arc<SecretSlice<u8>>>,
}

impl TlsPolicy {
    /// Creates secure defaults using platform-approved roots and TLS 1.2 or newer.
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            minimum_version: MinimumTlsVersion::Tls12,
            custom_roots: Vec::new(),
            client_identity: None,
        }
    }

    /// Requires a minimum TLS version.
    #[must_use]
    pub fn with_minimum_version(mut self, version: MinimumTlsVersion) -> Self {
        self.minimum_version = version;
        self
    }

    /// Adds one bounded PEM root certificate or bundle.
    ///
    /// # Errors
    ///
    /// Returns a validation error before transport construction for excessive material.
    pub fn with_custom_root_pem(
        mut self,
        pem: impl Into<Vec<u8>>,
    ) -> Result<Self, ValidationError> {
        let pem = pem.into();
        if pem.is_empty() || pem.len() > MAX_CUSTOM_ROOT_BYTES {
            return Err(validation(
                "network.tls.custom_roots",
                ValidationReason::OutOfRange,
                "custom CA material is empty or exceeds the SDK byte limit",
            ));
        }
        if self.custom_roots.len() >= MAX_CUSTOM_ROOTS {
            return Err(validation(
                "network.tls.custom_roots",
                ValidationReason::OutOfRange,
                "custom CA count exceeds the SDK limit",
            ));
        }
        self.custom_roots.push(Arc::from(pem));
        Ok(self)
    }

    /// Installs one bounded PEM client certificate and private-key identity.
    ///
    /// # Errors
    ///
    /// Returns a validation error for empty or excessive secret material.
    pub fn with_client_identity_pem(
        mut self,
        pem: impl Into<Vec<u8>>,
    ) -> Result<Self, ValidationError> {
        let pem = pem.into();
        if pem.is_empty() || pem.len() > MAX_CLIENT_IDENTITY_BYTES {
            return Err(validation(
                "network.tls.client_identity",
                ValidationReason::OutOfRange,
                "client identity is empty or exceeds the SDK byte limit",
            ));
        }
        self.client_identity = Some(Arc::new(SecretSlice::from(pem)));
        Ok(self)
    }

    /// Returns the configured minimum version.
    #[must_use]
    pub const fn minimum_version(&self) -> MinimumTlsVersion {
        self.minimum_version
    }

    pub(crate) fn custom_roots(&self) -> &[Arc<[u8]>] {
        &self.custom_roots
    }

    pub(crate) fn client_identity(&self) -> Option<&[u8]> {
        self.client_identity
            .as_ref()
            .map(|identity| identity.expose_secret())
    }
}

impl Default for TlsPolicy {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

impl fmt::Debug for TlsPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsPolicy")
            .field("minimum_version", &self.minimum_version)
            .field("custom_root_count", &self.custom_roots.len())
            .field("has_client_identity", &self.client_identity.is_some())
            .finish()
    }
}

/// Secure DNS timeout and address-family preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DnsPolicy {
    timeout: Duration,
    ip_preference: IpPreference,
}

impl DnsPolicy {
    /// Creates a five-second bounded resolver policy.
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            ip_preference: IpPreference::System,
        }
    }

    /// Replaces the DNS timeout.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the hard limit.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, ValidationError> {
        validate_duration("network.dns.timeout", timeout, MAX_DNS_TIMEOUT)?;
        self.timeout = timeout;
        Ok(self)
    }

    /// Replaces address-family ordering after filtering.
    #[must_use]
    pub fn with_ip_preference(mut self, preference: IpPreference) -> Self {
        self.ip_preference = preference;
        self
    }

    /// Returns the bounded lookup timeout.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// Returns address-family ordering.
    #[must_use]
    pub const fn ip_preference(self) -> IpPreference {
        self.ip_preference
    }
}

impl Default for DnsPolicy {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

/// Shared connection-pool and keepalive bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionPoolPolicy {
    idle_timeout: Duration,
    max_idle_per_host: usize,
    tcp_keepalive: Duration,
    http_version: HttpVersionPolicy,
}

impl ConnectionPoolPolicy {
    /// Creates bounded defaults for long-lived SDK clients.
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            idle_timeout: Duration::from_secs(90),
            max_idle_per_host: 8,
            tcp_keepalive: Duration::from_mins(1),
            http_version: HttpVersionPolicy::Negotiate,
        }
    }

    /// Replaces the idle connection timeout.
    pub fn with_idle_timeout(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_duration("network.pool.idle_timeout", value, MAX_POOL_IDLE_TIMEOUT)?;
        self.idle_timeout = value;
        Ok(self)
    }

    /// Replaces the maximum idle connections retained per origin.
    pub fn with_max_idle_per_host(mut self, value: usize) -> Result<Self, ValidationError> {
        if value > MAX_IDLE_PER_HOST {
            return Err(validation(
                "network.pool.max_idle_per_host",
                ValidationReason::OutOfRange,
                "idle connections per host exceed the SDK limit",
            ));
        }
        self.max_idle_per_host = value;
        Ok(self)
    }

    /// Replaces the TCP keepalive interval.
    pub fn with_tcp_keepalive(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_duration("network.pool.tcp_keepalive", value, MAX_POOL_IDLE_TIMEOUT)?;
        self.tcp_keepalive = value;
        Ok(self)
    }

    /// Replaces HTTP version negotiation policy.
    #[must_use]
    pub fn with_http_version(mut self, value: HttpVersionPolicy) -> Self {
        self.http_version = value;
        self
    }

    /// Returns idle timeout.
    #[must_use]
    pub const fn idle_timeout(self) -> Duration {
        self.idle_timeout
    }

    /// Returns the maximum idle connections per host.
    #[must_use]
    pub const fn max_idle_per_host(self) -> usize {
        self.max_idle_per_host
    }

    /// Returns TCP keepalive duration.
    #[must_use]
    pub const fn tcp_keepalive(self) -> Duration {
        self.tcp_keepalive
    }

    /// Returns HTTP version policy.
    #[must_use]
    pub const fn http_version(self) -> HttpVersionPolicy {
        self.http_version
    }
}

impl Default for ConnectionPoolPolicy {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

/// Complete immutable network policy used to build one bounded shared client pair.
#[derive(Clone, Debug, Default)]
pub struct NetworkPolicy {
    proxy: ProxyPolicy,
    tls: TlsPolicy,
    dns: DnsPolicy,
    pool: ConnectionPoolPolicy,
}

impl NetworkPolicy {
    /// Creates secure direct-connect defaults with TLS verification enabled.
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            proxy: ProxyPolicy::Disabled,
            tls: TlsPolicy::secure_defaults(),
            dns: DnsPolicy::secure_defaults(),
            pool: ConnectionPoolPolicy::secure_defaults(),
        }
    }

    /// Replaces deterministic proxy behavior.
    #[must_use]
    pub fn with_proxy(mut self, proxy: ProxyPolicy) -> Self {
        self.proxy = proxy;
        self
    }

    /// Replaces TLS roots, identity, and minimum version.
    #[must_use]
    pub fn with_tls(mut self, tls: TlsPolicy) -> Self {
        self.tls = tls;
        self
    }

    /// Replaces DNS timeout and address ordering.
    #[must_use]
    pub fn with_dns(mut self, dns: DnsPolicy) -> Self {
        self.dns = dns;
        self
    }

    /// Replaces connection-pool and HTTP negotiation bounds.
    #[must_use]
    pub fn with_pool(mut self, pool: ConnectionPoolPolicy) -> Self {
        self.pool = pool;
        self
    }

    /// Returns proxy policy.
    #[must_use]
    pub const fn proxy(&self) -> &ProxyPolicy {
        &self.proxy
    }

    /// Returns TLS policy.
    #[must_use]
    pub const fn tls(&self) -> &TlsPolicy {
        &self.tls
    }

    /// Returns DNS policy.
    #[must_use]
    pub const fn dns(&self) -> DnsPolicy {
        self.dns
    }

    /// Returns connection-pool policy.
    #[must_use]
    pub const fn pool(&self) -> ConnectionPoolPolicy {
        self.pool
    }
}

pub(crate) struct ResolvedProxy {
    pub(crate) endpoint: Url,
    pub(crate) credentials: Option<ProxyCredentials>,
    pub(crate) no_proxy: NoProxyList,
}

impl ProxyPolicy {
    pub(crate) fn resolve(&self) -> Result<Option<ResolvedProxy>, LlmError> {
        match self {
            Self::Disabled => Ok(None),
            Self::Explicit(proxy) => Ok(Some(ResolvedProxy {
                endpoint: proxy.endpoint().clone(),
                credentials: proxy.credentials().cloned(),
                no_proxy: proxy.no_proxy().clone(),
            })),
            Self::Environment => resolve_environment_proxy(),
        }
    }
}

fn resolve_environment_proxy() -> Result<Option<ResolvedProxy>, LlmError> {
    let raw = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let Some(raw) = raw else {
        return Ok(None);
    };
    let explicit = ExplicitProxy::new(&raw)?;
    let no_proxy_raw = ["NO_PROXY", "no_proxy"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let no_proxy = no_proxy_raw.map_or_else(
        || Ok(NoProxyList::default()),
        |raw| NoProxyList::new(raw.split(',').map(str::trim)),
    )?;
    Ok(Some(ResolvedProxy {
        endpoint: explicit.endpoint,
        credentials: None,
        no_proxy,
    }))
}

fn validate_duration(
    field: &'static str,
    value: Duration,
    maximum: Duration,
) -> Result<(), ValidationError> {
    if value.is_zero() {
        return Err(validation(
            field,
            ValidationReason::Zero,
            "network duration must be positive",
        ));
    }
    if value > maximum {
        return Err(validation(
            field,
            ValidationReason::OutOfRange,
            "network duration exceeds the SDK hard limit",
        ));
    }
    Ok(())
}

fn validation(
    field: &'static str,
    reason: ValidationReason,
    summary: &'static str,
) -> ValidationError {
    ValidationError::new(field, reason, summary)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ConnectionPoolPolicy, ExplicitProxy, NetworkPolicy, NoProxyList, ProxyCredentials,
        ProxyPolicy, TlsPolicy,
    };

    #[test]
    fn secure_defaults_are_direct_bounded_and_verified() {
        let policy = NetworkPolicy::secure_defaults();
        assert!(matches!(policy.proxy(), ProxyPolicy::Disabled));
        assert_eq!(policy.tls().custom_roots().len(), 0);
        assert_eq!(policy.pool().max_idle_per_host(), 8);
        assert!(policy.dns().timeout() <= Duration::from_secs(30));
    }

    #[test]
    fn proxy_credentials_and_client_identity_are_redacted() {
        let credentials = ProxyCredentials::new("proxy-user-canary", "proxy-pass-canary").unwrap();
        let proxy = ExplicitProxy::new("http://proxy.example:8080")
            .unwrap()
            .with_credentials(credentials)
            .with_no_proxy(NoProxyList::new(["example.com", "127.0.0.1"]).unwrap());
        let tls = TlsPolicy::secure_defaults()
            .with_client_identity_pem(b"private-key-canary".to_vec())
            .unwrap();
        let debug = format!(
            "{:?}",
            NetworkPolicy::secure_defaults()
                .with_proxy(ProxyPolicy::Explicit(proxy))
                .with_tls(tls)
        );
        assert!(!debug.contains("proxy-user-canary"));
        assert!(!debug.contains("proxy-pass-canary"));
        assert!(!debug.contains("private-key-canary"));
    }

    #[test]
    fn network_limits_reject_unbounded_configuration() {
        assert!(NoProxyList::new((0..65).map(|index| format!("host-{index}.example"))).is_err());
        assert!(
            ConnectionPoolPolicy::secure_defaults()
                .with_max_idle_per_host(65)
                .is_err()
        );
        assert!(ExplicitProxy::new("http://user:pass@proxy.example").is_err());
    }
}
