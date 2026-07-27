//! Credential destination restrictions.

use std::fmt;

use crate::error::LlmError;

use super::{Origin, ResolvedEndpoint};

/// Credential destination restriction.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CredentialAudience {
    /// Official `OpenAI` API at `https://api.openai.com:443`.
    OfficialOpenAi,
    /// Official Anthropic API at `https://api.anthropic.com:443`.
    OfficialAnthropic,
    /// `OpenRouter` API at `https://openrouter.ai:443`.
    OpenRouterApi,
    /// `DeepSeek` API at `https://api.deepseek.com:443`.
    DeepSeekApi,
    /// Z.AI standard `PaaS` API at `https://api.z.ai:443`.
    ZaiStandard,
    /// Z.AI Coding Plan API at `https://api.z.ai:443`.
    ZaiCoding,
    /// Exact origin used only by the explicit test profile.
    #[doc(hidden)]
    TestOnlyExactOrigin(Origin),
}

impl CredentialAudience {
    /// Validates that a credential may be sent to the endpoint.
    pub fn validate(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        CredentialBinding::from(self).validate(endpoint)
    }
}

/// Immutable credential destination bound to a normalized endpoint origin.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CredentialBinding {
    origin: Origin,
    required_path_prefix: Option<&'static str>,
    label: &'static str,
}

impl CredentialBinding {
    /// Binds a credential to the exact origin of a validated production HTTPS endpoint.
    pub fn exact_https_origin(endpoint: &ResolvedEndpoint) -> Result<Self, LlmError> {
        if endpoint.origin().scheme() != crate::protected::REQUIRED_ENDPOINT_SCHEME {
            return Err(LlmError::Configuration(
                "production credential binding requires an HTTPS endpoint".to_owned(),
            ));
        }
        Ok(Self {
            origin: endpoint.origin().clone(),
            required_path_prefix: None,
            label: "exact-https-origin",
        })
    }

    /// Returns the fixed official `OpenAI` credential destination.
    #[must_use]
    pub fn official_openai() -> Self {
        Self::official("api.openai.com", "official-openai")
    }

    /// Returns the fixed official Anthropic credential destination.
    #[must_use]
    pub fn official_anthropic() -> Self {
        Self::official("api.anthropic.com", "official-anthropic")
    }

    /// Validates that the binding permits sending a credential to the endpoint.
    pub fn validate(&self, endpoint: &ResolvedEndpoint) -> Result<(), LlmError> {
        let allowed = self.origin == *endpoint.origin()
            && self
                .required_path_prefix
                .is_none_or(|prefix| endpoint.url().path().starts_with(prefix));
        if allowed {
            Ok(())
        } else {
            Err(LlmError::Configuration(
                "credential binding does not match endpoint origin".to_owned(),
            ))
        }
    }

    pub(crate) fn exact_origin_for_test(origin: Origin) -> Self {
        Self {
            origin,
            required_path_prefix: None,
            label: "test-only-exact-origin",
        }
    }

    fn official(host: &str, label: &'static str) -> Self {
        Self {
            origin: Origin::new("https", host, 443),
            required_path_prefix: None,
            label,
        }
    }

    fn official_with_path(
        host: &str,
        required_path_prefix: &'static str,
        label: &'static str,
    ) -> Self {
        Self {
            origin: Origin::new("https", host, 443),
            required_path_prefix: Some(required_path_prefix),
            label,
        }
    }
}

impl fmt::Debug for CredentialBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialBinding")
            .field("label", &self.label)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl From<CredentialAudience> for CredentialBinding {
    fn from(audience: CredentialAudience) -> Self {
        Self::from(&audience)
    }
}

impl From<&CredentialAudience> for CredentialBinding {
    fn from(audience: &CredentialAudience) -> Self {
        match audience {
            CredentialAudience::OfficialOpenAi => Self::official_openai(),
            CredentialAudience::OfficialAnthropic => Self::official_anthropic(),
            CredentialAudience::OpenRouterApi => Self::official("openrouter.ai", "openrouter"),
            CredentialAudience::DeepSeekApi => Self::official("api.deepseek.com", "deepseek"),
            CredentialAudience::ZaiStandard => {
                Self::official_with_path("api.z.ai", "/api/paas/v4/", "zai-standard")
            }
            CredentialAudience::ZaiCoding => {
                Self::official_with_path("api.z.ai", "/api/coding/paas/v4/", "zai-coding")
            }
            CredentialAudience::TestOnlyExactOrigin(origin) => {
                Self::exact_origin_for_test(origin.clone())
            }
        }
    }
}

impl From<&CredentialBinding> for CredentialBinding {
    fn from(binding: &CredentialBinding) -> Self {
        binding.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::endpoint::{EndpointConfig, resolve_official};

    #[test]
    fn exact_https_binding_accepts_paths_only_within_one_origin() {
        let first = resolve_official(
            &EndpointConfig::absolute("https://llm.example.com/v1/messages").unwrap(),
        )
        .unwrap();
        let second =
            resolve_official(&EndpointConfig::absolute("https://llm.example.com/v2/chat").unwrap())
                .unwrap();
        let foreign = resolve_official(
            &EndpointConfig::absolute("https://other.example.com/v1/messages").unwrap(),
        )
        .unwrap();
        let binding = CredentialBinding::exact_https_origin(&first).unwrap();
        assert!(binding.validate(&second).is_ok());
        assert!(binding.validate(&foreign).is_err());

        let other_port = resolve_official(
            &EndpointConfig::absolute("https://llm.example.com:8443/v1/messages").unwrap(),
        )
        .unwrap();
        assert!(binding.validate(&other_port).is_err());
    }

    #[test]
    fn production_binding_rejects_http_and_debug_is_value_free() {
        let endpoint = crate::provider::endpoint::resolve_test_only(
            &EndpointConfig::absolute("http://127.0.0.1:8787/v1/messages").unwrap(),
        )
        .unwrap();
        assert!(CredentialBinding::exact_https_origin(&endpoint).is_err());

        let binding = CredentialBinding::official_openai();
        let debug = format!("{binding:?}");
        assert!(debug.contains("official-openai"));
        assert!(!debug.contains("/v1"));
    }
}
