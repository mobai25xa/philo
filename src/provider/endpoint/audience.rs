//! Credential destination restrictions.

use crate::error::LlmError;

use super::{Origin, ResolvedEndpoint};

/// Credential destination restriction.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CredentialAudience {
    /// Official `OpenAI` API at `https://api.openai.com:443`.
    OfficialOpenAi,
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
        let allowed = match self {
            Self::OfficialOpenAi => {
                endpoint.origin().scheme() == "https"
                    && endpoint.origin().host() == "api.openai.com"
                    && endpoint.origin().port() == 443
            }
            Self::OpenRouterApi => exact_https_origin(endpoint, "openrouter.ai"),
            Self::DeepSeekApi => exact_https_origin(endpoint, "api.deepseek.com"),
            Self::ZaiStandard => {
                exact_https_origin(endpoint, "api.z.ai")
                    && endpoint.url().path().starts_with("/api/paas/v4/")
            }
            Self::ZaiCoding => {
                exact_https_origin(endpoint, "api.z.ai")
                    && endpoint.url().path().starts_with("/api/coding/paas/v4/")
            }
            Self::TestOnlyExactOrigin(origin) => origin == endpoint.origin(),
        };
        if allowed {
            Ok(())
        } else {
            Err(LlmError::Configuration(
                "credential audience does not match endpoint origin".to_owned(),
            ))
        }
    }
}

fn exact_https_origin(endpoint: &ResolvedEndpoint, host: &str) -> bool {
    endpoint.origin().scheme() == "https"
        && endpoint.origin().host() == host
        && endpoint.origin().port() == 443
}
