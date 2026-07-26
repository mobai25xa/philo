//! Network-target and redirect policy.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

use crate::error::LlmError;

use super::{CredentialBinding, EndpointMode, EndpointResolutionDiagnostics, ResolvedEndpoint};

/// Endpoint network class and optional exact-host allowlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointNetworkPolicy {
    mode: EndpointMode,
    allowed_hosts: BTreeSet<String>,
}

impl EndpointNetworkPolicy {
    /// Requires HTTPS and rejects literal non-public addresses, localhost names, and IDN hosts.
    #[must_use]
    pub const fn public_https() -> Self {
        Self {
            mode: EndpointMode::Production,
            allowed_hosts: BTreeSet::new(),
        }
    }

    /// Restricts a production policy to one additional exact normalized hostname.
    pub fn with_allowed_host(mut self, host: impl Into<String>) -> Result<Self, LlmError> {
        let host = host.into().to_ascii_lowercase();
        if host.is_empty() || !host.is_ascii() || host.contains(['/', ':', '@']) {
            return Err(configuration("invalid endpoint allowlist host"));
        }
        self.allowed_hosts.insert(host);
        Ok(self)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn test_loopback() -> Self {
        Self {
            mode: EndpointMode::TestOnly,
            allowed_hosts: BTreeSet::new(),
        }
    }

    pub(crate) fn for_mode(mode: EndpointMode) -> Self {
        match mode {
            EndpointMode::Production => Self::public_https(),
            EndpointMode::TestOnly => Self::test_loopback(),
        }
    }

    /// Validates a URL before it can become a resolved endpoint.
    pub fn validate(&self, url: &Url) -> Result<(), LlmError> {
        validate_common(url)?;
        let host = url
            .host()
            .ok_or_else(|| configuration("endpoint must include a host"))?;
        match self.mode {
            EndpointMode::Production => {
                if url.scheme() != "https" {
                    return Err(configuration("production endpoint requires HTTPS"));
                }
                validate_public_host(&host)?;
            }
            EndpointMode::TestOnly => {
                if !matches!(url.scheme(), "http" | "https") || !is_loopback(&host) {
                    return Err(configuration(
                        "test endpoint must use HTTP(S) on a loopback host",
                    ));
                }
            }
        }
        if !self.allowed_hosts.is_empty() {
            let normalized = host_to_string(&host);
            if !self.allowed_hosts.contains(&normalized) {
                return Err(configuration("endpoint host is not allowlisted"));
            }
        }
        Ok(())
    }

    /// Validates every address returned by DNS before the HTTP connector uses it.
    pub fn validate_resolved_addresses<I>(&self, addresses: I) -> Result<(), LlmError>
    where
        I: IntoIterator<Item = IpAddr>,
    {
        let addresses = addresses.into_iter().collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(configuration("DNS resolution returned no addresses"));
        }
        let allowed = addresses.into_iter().all(|address| match self.mode {
            EndpointMode::Production => match address {
                IpAddr::V4(address) => is_public_ipv4(address),
                IpAddr::V6(address) => is_public_ipv6(address),
            },
            EndpointMode::TestOnly => address.is_loopback(),
        });
        if allowed {
            Ok(())
        } else {
            Err(configuration(
                "DNS resolution included a forbidden network address",
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
        self.validate_inner(from, to, None).map(|_| ())
    }

    /// Revalidates URL policy and credential audience for one redirect hop.
    pub fn validate_hop(
        self,
        from: &ResolvedEndpoint,
        to: &Url,
        binding: impl Into<CredentialBinding>,
    ) -> Result<ResolvedEndpoint, LlmError> {
        let binding = binding.into();
        self.validate_inner(from, to, Some(&binding))
    }

    fn validate_inner(
        self,
        from: &ResolvedEndpoint,
        to: &Url,
        binding: Option<&CredentialBinding>,
    ) -> Result<ResolvedEndpoint, LlmError> {
        if self == Self::Disabled {
            return Err(configuration("redirects are disabled"));
        }
        if to.query().is_some() {
            return Err(configuration("redirect query is forbidden"));
        }
        let policy = if from.origin().scheme() == "https" && !is_loopback_url(from.url()) {
            EndpointNetworkPolicy::public_https()
        } else {
            EndpointNetworkPolicy::test_loopback()
        };
        policy.validate(to)?;
        let target = ResolvedEndpoint::new(to.clone(), EndpointResolutionDiagnostics::default())?;
        if target.origin() != from.origin() {
            return Err(configuration("cross-origin redirect is forbidden"));
        }
        if from.origin().scheme() == "https" && target.origin().scheme() != "https" {
            return Err(configuration("HTTPS redirect downgrade is forbidden"));
        }
        if let Some(binding) = binding {
            binding.validate(&target)?;
        }
        Ok(target)
    }
}

pub(crate) fn validate_common(url: &Url) -> Result<(), LlmError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(configuration("endpoint userinfo is forbidden"));
    }
    if url.fragment().is_some() {
        return Err(configuration("endpoint fragment is forbidden"));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(configuration("endpoint scheme must be HTTP(S)"));
    }
    if url.host().is_none() {
        return Err(configuration("endpoint must include a host"));
    }
    Ok(())
}

fn validate_public_host(host: &Host<&str>) -> Result<(), LlmError> {
    let allowed = match host {
        Host::Domain(name) => {
            let lower = name.to_ascii_lowercase();
            !lower.eq("localhost")
                && !lower.ends_with(".localhost")
                && !is_local_domain(&lower)
                && !lower.starts_with("xn--")
                && !lower.contains(".xn--")
        }
        Host::Ipv4(address) => is_public_ipv4(*address),
        Host::Ipv6(address) => is_public_ipv6(*address),
    };
    if allowed {
        Ok(())
    } else {
        Err(configuration(
            "production endpoint must not target private, loopback, link-local, or IDN hosts",
        ))
    }
}

fn is_local_domain(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("local"))
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    !(address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address == Ipv4Addr::BROADCAST
        || is_ipv4_range(address, [100, 64, 0, 0], 10)
        || is_ipv4_range(address, [192, 0, 0, 0], 24)
        || is_ipv4_range(address, [198, 18, 0, 0], 15)
        || is_ipv4_range(address, [240, 0, 0, 0], 4))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let first = address.segments()[0];
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
        || (address.segments()[0] == 0x2001 && address.segments()[1] == 0x0db8))
}

fn is_ipv4_range(address: Ipv4Addr, network: [u8; 4], prefix: u32) -> bool {
    let value = u32::from(address);
    let network = u32::from(Ipv4Addr::from(network));
    let mask = u32::MAX << (32 - prefix);
    value & mask == network & mask
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

fn is_loopback_url(url: &Url) -> bool {
    url.host().is_some_and(|host| is_loopback(&host))
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
