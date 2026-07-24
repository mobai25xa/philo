//! Local, deterministic, low-priority endpoint detection suggestions.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::net::IpAddr;

use url::{Host, Url};

use crate::domain::ProviderId;
use crate::error::{ValidationError, ValidationReason};

use super::catalog::ProductId;

/// Whether endpoint detection may be considered by the provider selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EndpointDetectionPolicy {
    /// Consider reviewed built-in endpoint rules only after every explicit source.
    #[default]
    Enabled,
    /// Do not run endpoint detection.
    Disabled,
}

/// Sanitized endpoint facts admitted to the detector.
#[derive(Clone, Eq, PartialEq)]
pub struct NormalizedEndpointFacts {
    scheme: String,
    host: String,
    path: String,
    port: Option<u16>,
    ip_literal: bool,
    idn: bool,
    userinfo_present: bool,
}

impl NormalizedEndpointFacts {
    /// Parses an endpoint and retains no query or fragment values.
    pub fn parse(endpoint: &str) -> Result<Self, ValidationError> {
        let url = Url::parse(endpoint).map_err(|_| {
            ValidationError::new(
                "endpoint_detection.endpoint",
                ValidationReason::InvalidIdentifier,
                "endpoint must be an absolute URL",
            )
        })?;
        let host = url.host().ok_or_else(|| {
            ValidationError::new(
                "endpoint_detection.host",
                ValidationReason::InvalidIdentifier,
                "endpoint must contain a host",
            )
        })?;
        let (host, ip_literal) = match host {
            Host::Domain(domain) => (domain.trim_end_matches('.').to_ascii_lowercase(), false),
            Host::Ipv4(address) => (IpAddr::V4(address).to_string(), true),
            Host::Ipv6(address) => (IpAddr::V6(address).to_string(), true),
        };
        let idn = host
            .split('.')
            .any(|label| label.starts_with("xn--") || !label.is_ascii());
        let path = normalize_path(url.path());
        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host,
            path,
            port: url.port(),
            ip_literal,
            idn,
            userinfo_present: !url.username().is_empty() || url.password().is_some(),
        })
    }

    /// Returns the normalized scheme.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the normalized, non-sensitive host fact.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the normalized product path. Query and fragment are never retained.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns a non-default explicit port, if present.
    pub const fn port(&self) -> Option<u16> {
        self.port
    }
}

impl fmt::Debug for NormalizedEndpointFacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedEndpointFacts")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("path", &self.path)
            .field("port", &self.port)
            .field("ip_literal", &self.ip_literal)
            .field("idn", &self.idn)
            .field("userinfo_present", &self.userinfo_present)
            .finish()
    }
}

/// Stable confidence class for a reviewed rule match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectionConfidence {
    /// Exact hostname and exact product path match.
    Exact,
    /// DNS-label-boundary suffix and exact product path match.
    SuffixSafe,
}

/// Why no endpoint suggestion was produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectionUnknownReason {
    /// Detection was explicitly disabled.
    Disabled,
    /// Endpoint facts were absent.
    MissingEndpoint,
    /// IP literals are never assigned to a provider.
    IpLiteral,
    /// IDN/punycode hosts are not present in the reviewed built-in rules.
    InternationalizedHost,
    /// User information invalidates a detection candidate.
    UserInfo,
    /// No reviewed host/product rule matched.
    NoRule,
}

/// Value-free explanation for a suggestion or unknown result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionExplanation {
    rule_id: Option<&'static str>,
    rule_owner: Option<&'static str>,
    reviewed_at: Option<&'static str>,
    host: Option<String>,
    product_path: Option<String>,
    confidence: Option<DetectionConfidence>,
    unknown_reason: Option<DetectionUnknownReason>,
}

impl DetectionExplanation {
    /// Returns the stable rule ID, if matched.
    pub const fn rule_id(&self) -> Option<&'static str> {
        self.rule_id
    }

    /// Returns the stable rule owner, if matched.
    pub const fn rule_owner(&self) -> Option<&'static str> {
        self.rule_owner
    }

    /// Returns the rule review date, if matched.
    pub const fn reviewed_at(&self) -> Option<&'static str> {
        self.reviewed_at
    }

    /// Returns the normalized host fact, never a full URL.
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Returns the normalized product path without query or fragment.
    pub fn product_path(&self) -> Option<&str> {
        self.product_path.as_deref()
    }

    /// Returns match confidence.
    pub const fn confidence(&self) -> Option<DetectionConfidence> {
        self.confidence
    }

    /// Returns the unknown reason, if no rule matched.
    pub const fn unknown_reason(&self) -> Option<DetectionUnknownReason> {
        self.unknown_reason
    }
}

/// Low-authority provider/product suggestion from endpoint facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionSuggestion {
    provider_id: ProviderId,
    product_id: ProductId,
    explanation: DetectionExplanation,
}

impl DetectionSuggestion {
    /// Returns the suggested provider.
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the suggested product.
    pub const fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the value-free rule explanation.
    pub const fn explanation(&self) -> &DetectionExplanation {
        &self.explanation
    }
}

/// Pure detector result. A suggestion has no authority until a factory adopts it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointDetection {
    /// A reviewed built-in rule matched.
    Suggested(DetectionSuggestion),
    /// Detection remained unknown.
    Unknown(DetectionExplanation),
}

/// Reviewed built-in endpoint detector. It performs no DNS or HTTP work.
#[derive(Clone, Copy, Debug, Default)]
pub struct EndpointDetector;

impl EndpointDetector {
    /// Applies the explicit enable/disable policy before evaluating rules.
    pub fn detect_with_policy(
        policy: EndpointDetectionPolicy,
        facts: Option<&NormalizedEndpointFacts>,
    ) -> EndpointDetection {
        match policy {
            EndpointDetectionPolicy::Enabled => Self::detect(facts),
            EndpointDetectionPolicy::Disabled => unknown(facts, DetectionUnknownReason::Disabled),
        }
    }

    /// Evaluates built-in rules against sanitized endpoint facts.
    pub fn detect(facts: Option<&NormalizedEndpointFacts>) -> EndpointDetection {
        let Some(facts) = facts else {
            return unknown(None, DetectionUnknownReason::MissingEndpoint);
        };
        if facts.userinfo_present {
            return unknown(Some(facts), DetectionUnknownReason::UserInfo);
        }
        if facts.ip_literal {
            return unknown(Some(facts), DetectionUnknownReason::IpLiteral);
        }
        if facts.idn {
            return unknown(Some(facts), DetectionUnknownReason::InternationalizedHost);
        }
        for rule in RULES {
            if let Some(confidence) = rule.matches(facts) {
                let (Ok(provider_id), Ok(product_id)) = (
                    ProviderId::new(rule.provider_id),
                    ProductId::new(rule.product_id),
                ) else {
                    continue;
                };
                return EndpointDetection::Suggested(DetectionSuggestion {
                    provider_id,
                    product_id,
                    explanation: DetectionExplanation {
                        rule_id: Some(rule.id),
                        rule_owner: Some(rule.owner),
                        reviewed_at: Some(rule.reviewed_at),
                        host: Some(facts.host.clone()),
                        product_path: Some(facts.path.clone()),
                        confidence: Some(confidence),
                        unknown_reason: None,
                    },
                });
            }
        }
        unknown(Some(facts), DetectionUnknownReason::NoRule)
    }
}

#[derive(Clone, Copy)]
enum HostRule {
    Exact(&'static str),
    #[allow(dead_code)]
    Suffix(&'static str),
}

struct DetectionRule {
    id: &'static str,
    owner: &'static str,
    reviewed_at: &'static str,
    host: HostRule,
    path: &'static str,
    provider_id: &'static str,
    product_id: &'static str,
}

impl DetectionRule {
    fn matches(&self, facts: &NormalizedEndpointFacts) -> Option<DetectionConfidence> {
        if facts.path != self.path || facts.scheme != "https" || facts.port.is_some() {
            return None;
        }
        match self.host {
            HostRule::Exact(host) if facts.host == host => Some(DetectionConfidence::Exact),
            HostRule::Suffix(suffix) if suffix_match(&facts.host, suffix) => {
                Some(DetectionConfidence::SuffixSafe)
            }
            HostRule::Exact(_) | HostRule::Suffix(_) => None,
        }
    }
}

const RULES: &[DetectionRule] = &[DetectionRule {
    id: "builtin.official-openai.chat-completions.v1",
    owner: "philo/provider",
    reviewed_at: "2026-07-24",
    host: HostRule::Exact("api.openai.com"),
    path: "/v1/chat/completions",
    provider_id: "official-openai",
    product_id: "chat-completions",
}];

fn suffix_match(host: &str, suffix: &str) -> bool {
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
}

fn normalize_path(path: &str) -> String {
    let mut path = if path.is_empty() { "/" } else { path }.to_owned();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    path
}

fn unknown(
    facts: Option<&NormalizedEndpointFacts>,
    reason: DetectionUnknownReason,
) -> EndpointDetection {
    EndpointDetection::Unknown(DetectionExplanation {
        rule_id: None,
        rule_owner: None,
        reviewed_at: None,
        host: facts.map(|value| value.host.clone()),
        product_path: facts.map(|value| value.path.clone()),
        confidence: None,
        unknown_reason: Some(reason),
    })
}

#[cfg(test)]
mod tests {
    use super::suffix_match;

    #[test]
    fn suffix_matching_requires_dns_label_boundary() {
        assert!(suffix_match("api.example.com", "example.com"));
        assert!(suffix_match("example.com", "example.com"));
        assert!(!suffix_match("evil-example.com", "example.com"));
        assert!(!suffix_match("notexample.com", "example.com"));
    }
}
