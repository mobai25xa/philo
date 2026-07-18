//! Endpoint resolution, origin comparison, and credential-audience enforcement.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use url::{Host, Url};

use crate::error::LlmError;

/// Mutually exclusive endpoint configuration inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointConfig {
    /// Resolve an endpoint by appending a path to a base URL.
    BaseAndPath {
        /// Base API URL, including any prefix such as `/v1`.
        base_url: Url,
        /// Endpoint path appended to the base path.
        endpoint_path: String,
    },
    /// Use an already absolute endpoint.
    Absolute(Url),
}

impl EndpointConfig {
    /// Parses a base URL and stores a path for later validation.
    pub fn base_and_path(
        base_url: &str,
        endpoint_path: impl Into<String>,
    ) -> Result<Self, LlmError> {
        let base_url = Url::parse(base_url).map_err(|_| configuration("invalid base URL"))?;
        Ok(Self::BaseAndPath {
            base_url,
            endpoint_path: endpoint_path.into(),
        })
    }

    /// Parses an absolute endpoint for later validation.
    pub fn absolute(endpoint: &str) -> Result<Self, LlmError> {
        let url = Url::parse(endpoint).map_err(|_| configuration("invalid absolute endpoint"))?;
        Ok(Self::Absolute(url))
    }
}

/// Scheme, host, and effective port used for same-origin checks.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    /// Extracts a normalized origin from a URL.
    pub fn from_url(url: &Url) -> Result<Self, LlmError> {
        let host = url
            .host()
            .ok_or_else(|| configuration("endpoint must include a host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| configuration("endpoint scheme has no effective port"))?;
        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host_to_string(&host),
            port,
        })
    }
    /// Returns the normalized scheme.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    /// Returns the normalized host.
    pub fn host(&self) -> &str {
        &self.host
    }
    /// Returns the effective port.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A fully validated endpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedEndpoint {
    url: Url,
    origin: Origin,
}

impl ResolvedEndpoint {
    /// Returns the final URL.
    pub fn url(&self) -> &Url {
        &self.url
    }
    /// Returns the final origin.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

impl fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field("url", &self.url.as_str())
            .field("origin", &self.origin)
            .finish()
    }
}

/// Credential destination restriction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialAudience {
    /// Official `OpenAI` API at `https://api.openai.com:443`.
    OfficialOpenAi,
    /// Exact origin used only by the explicit test profile.
    #[doc(hidden)]
    TestOnlyExactOrigin(Origin),
}

impl CredentialAudience {
    /// Validates that a credential may be sent to the endpoint.
    pub fn validate(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        let allowed = match self {
            Self::OfficialOpenAi => {
                endpoint.origin.scheme == "https"
                    && endpoint.origin.host == "api.openai.com"
                    && endpoint.origin.port == 443
            }
            Self::TestOnlyExactOrigin(origin) => origin == endpoint.origin(),
        };
        if allowed {
            Ok(())
        } else {
            Err(configuration(
                "credential audience does not match endpoint origin",
            ))
        }
    }
}

/// Redirect policy for credential-bearing requests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RedirectPolicy {
    /// Reject every redirect.
    #[default]
    Disabled,
    /// Permit redirects only to the same scheme, host, and effective port.
    SameOrigin,
}

impl RedirectPolicy {
    /// Checks a proposed redirect without performing network I/O.
    pub fn validate(self, from: &ResolvedEndpoint, to: &Url) -> Result<(), LlmError> {
        if self == Self::Disabled {
            return Err(configuration("redirects are disabled"));
        }
        validate_url(to, EndpointMode::OfficialOrTest)?;
        let target_origin = Origin::from_url(to)?;
        if target_origin != *from.origin() {
            return Err(configuration("cross-origin redirect is forbidden"));
        }
        if from.origin().scheme() == "https" && target_origin.scheme() != "https" {
            return Err(configuration("HTTPS redirect downgrade is forbidden"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum EndpointMode {
    Official,
    TestOnly,
    OfficialOrTest,
}

/// Resolves and validates the official endpoint input.
pub fn resolve_official(config: &EndpointConfig) -> Result<ResolvedEndpoint, LlmError> {
    resolve(config, EndpointMode::Official)
}

/// Resolves a localhost-only test endpoint.
#[doc(hidden)]
pub fn resolve_test_only(config: &EndpointConfig) -> Result<ResolvedEndpoint, LlmError> {
    resolve(config, EndpointMode::TestOnly)
}

fn resolve(config: &EndpointConfig, mode: EndpointMode) -> Result<ResolvedEndpoint, LlmError> {
    let url = match config {
        EndpointConfig::Absolute(url) => url.clone(),
        EndpointConfig::BaseAndPath {
            base_url,
            endpoint_path,
        } => append_path(base_url, endpoint_path)?,
    };
    validate_url(&url, mode)?;
    let origin = Origin::from_url(&url)?;
    Ok(ResolvedEndpoint { url, origin })
}

fn append_path(base: &Url, endpoint_path: &str) -> Result<Url, LlmError> {
    if endpoint_path.is_empty() {
        return Err(configuration("endpoint path must not be empty"));
    }
    if endpoint_path.contains(['?', '#']) {
        return Err(configuration(
            "endpoint path must not contain query or fragment",
        ));
    }
    let mut result = base.clone();
    let base_path = base.path().trim_end_matches('/');
    let endpoint_path = endpoint_path.trim_start_matches('/');
    let combined = if base_path.is_empty() {
        format!("/{endpoint_path}")
    } else {
        format!("{base_path}/{endpoint_path}")
    };
    result.set_path(&combined);
    Ok(result)
}

fn validate_url(url: &Url, mode: EndpointMode) -> Result<(), LlmError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(configuration("endpoint userinfo is forbidden"));
    }
    if url.fragment().is_some() {
        return Err(configuration("endpoint fragment is forbidden"));
    }
    if url.query().is_some() {
        return Err(configuration("endpoint query is forbidden"));
    }
    let host = url
        .host()
        .ok_or_else(|| configuration("endpoint must include a host"))?;
    match mode {
        EndpointMode::Official if url.scheme() == "https" => Ok(()),
        EndpointMode::Official => Err(configuration("official endpoint requires HTTPS")),
        EndpointMode::TestOnly
            if matches!(url.scheme(), "http" | "https") && is_loopback(&host) =>
        {
            Ok(())
        }
        EndpointMode::TestOnly => Err(configuration(
            "test endpoint must use HTTP(S) on a loopback host",
        )),
        EndpointMode::OfficialOrTest if matches!(url.scheme(), "http" | "https") => Ok(()),
        EndpointMode::OfficialOrTest => Err(configuration("redirect scheme must be HTTP(S)")),
    }
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

fn host_to_string(host: &Host<&str>) -> String {
    match host {
        Host::Domain(name) => name.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => address.to_string(),
    }
}

fn configuration(message: &'static str) -> LlmError {
    LlmError::Configuration(message.to_owned())
}
