//! Endpoint configuration and the single resolver implementation.

use std::fmt;

use url::Url;

use crate::error::LlmError;

use super::{
    EndpointNetworkPolicy, EndpointQuery, EndpointResolutionDiagnostics, EndpointTemplate,
    EndpointValues, ResolvedEndpoint,
};

/// Network mode selected by a trusted provider preset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointMode {
    Production,
    TestOnly,
}

/// Mutually exclusive endpoint configuration inputs.
#[derive(Clone, Eq, PartialEq)]
pub enum EndpointConfig {
    /// Resolve an endpoint by appending a fixed path to a base URL.
    BaseAndPath {
        /// Base API URL, including any prefix such as `/v1`.
        base_url: Url,
        /// Endpoint path appended to the base path.
        endpoint_path: String,
    },
    /// Resolve a restricted typed template and registered query plan.
    Template {
        /// Base API URL, including a proxy prefix and only registered query keys.
        base_url: Url,
        /// Restricted path template.
        template: EndpointTemplate,
        /// Explicit query merge plan.
        query: EndpointQuery,
    },
    /// Use an already absolute endpoint.
    Absolute(Url),
}

impl fmt::Debug for EndpointConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaseAndPath {
                base_url,
                endpoint_path,
            } => formatter
                .debug_struct("EndpointConfig::BaseAndPath")
                .field("base_origin", &base_url.origin().ascii_serialization())
                .field("base_path", &base_url.path())
                .field("endpoint_path", endpoint_path)
                .finish(),
            Self::Template {
                base_url,
                template,
                query,
            } => formatter
                .debug_struct("EndpointConfig::Template")
                .field("base_origin", &base_url.origin().ascii_serialization())
                .field("base_path", &base_url.path())
                .field("template", template)
                .field("query", query)
                .finish(),
            Self::Absolute(url) => formatter
                .debug_struct("EndpointConfig::Absolute")
                .field("origin", &url.origin().ascii_serialization())
                .field("path", &url.path())
                .finish(),
        }
    }
}

impl EndpointConfig {
    /// Parses a base URL and stores a fixed path for later validation.
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

    /// Parses a base URL and stores a restricted target-aware template.
    pub fn base_and_template(
        base_url: &str,
        template: EndpointTemplate,
        query: EndpointQuery,
    ) -> Result<Self, LlmError> {
        let base_url = Url::parse(base_url).map_err(|_| configuration("invalid base URL"))?;
        Ok(Self::Template {
            base_url,
            template,
            query,
        })
    }

    /// Parses an absolute endpoint for later validation.
    pub fn absolute(endpoint: &str) -> Result<Self, LlmError> {
        let url = Url::parse(endpoint).map_err(|_| configuration("invalid absolute endpoint"))?;
        Ok(Self::Absolute(url))
    }

    pub(crate) fn requires_mapping(&self) -> bool {
        matches!(
            self,
            Self::Template { template, .. } if template.requires_values()
        )
    }

    pub(crate) fn resolve(
        &self,
        mode: EndpointMode,
        values: Option<EndpointValues<'_>>,
    ) -> Result<ResolvedEndpoint, LlmError> {
        let (url, diagnostics) = match self {
            Self::Absolute(url) => {
                if url.query().is_some() {
                    return Err(configuration("absolute endpoint query is forbidden"));
                }
                (url.clone(), EndpointResolutionDiagnostics::default())
            }
            Self::BaseAndPath {
                base_url,
                endpoint_path,
            } => {
                if base_url.query().is_some() {
                    return Err(configuration("fixed base endpoint query is forbidden"));
                }
                let template = EndpointTemplate::parse(endpoint_path)?;
                let (path, used) = template.render(base_url.path(), None)?;
                let mut url = base_url.clone();
                url.set_path(&path);
                (url, EndpointResolutionDiagnostics::new(used, Vec::new()))
            }
            Self::Template {
                base_url,
                template,
                query,
            } => {
                let (path, used) = template.render(base_url.path(), values)?;
                let mut url = base_url.clone();
                url.set_path(&path);
                let query_diagnostics = query.apply(&mut url)?;
                (
                    url,
                    EndpointResolutionDiagnostics::new(used, query_diagnostics),
                )
            }
        };
        EndpointNetworkPolicy::for_mode(mode).validate(&url)?;
        ResolvedEndpoint::new(url, diagnostics)
    }
}

/// Resolves and validates the official endpoint input.
pub fn resolve_official(config: &EndpointConfig) -> Result<ResolvedEndpoint, LlmError> {
    config.resolve(EndpointMode::Production, None)
}

/// Resolves an official target-aware endpoint.
pub fn resolve_official_for(
    config: &EndpointConfig,
    values: EndpointValues<'_>,
) -> Result<ResolvedEndpoint, LlmError> {
    config.resolve(EndpointMode::Production, Some(values))
}

/// Resolves a localhost-only test endpoint.
#[doc(hidden)]
pub fn resolve_test_only(config: &EndpointConfig) -> Result<ResolvedEndpoint, LlmError> {
    config.resolve(EndpointMode::TestOnly, None)
}

#[doc(hidden)]
pub(crate) fn resolve_test_only_for(
    config: &EndpointConfig,
    values: EndpointValues<'_>,
) -> Result<ResolvedEndpoint, LlmError> {
    config.resolve(EndpointMode::TestOnly, Some(values))
}

fn configuration(message: &'static str) -> LlmError {
    LlmError::Configuration(message.to_owned())
}
