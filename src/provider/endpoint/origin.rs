//! Normalized origins and immutable resolved endpoints.

use std::fmt;

use url::{Host, Url};

use crate::error::LlmError;

use super::{EndpointPathVariable, EndpointQueryDiagnostic};

/// Scheme, host, and effective port used for same-origin checks.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    pub(crate) fn new(scheme: &str, host: &str, port: u16) -> Self {
        Self {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port,
        }
    }

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

/// Value-free trace of how one endpoint was resolved.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndpointResolutionDiagnostics {
    path_variables: Vec<EndpointPathVariable>,
    query: Vec<EndpointQueryDiagnostic>,
}

impl EndpointResolutionDiagnostics {
    pub(crate) fn new(
        path_variables: Vec<EndpointPathVariable>,
        query: Vec<EndpointQueryDiagnostic>,
    ) -> Self {
        Self {
            path_variables,
            query,
        }
    }

    /// Returns the typed path variables used, without their values.
    pub fn path_variables(&self) -> &[EndpointPathVariable] {
        &self.path_variables
    }

    /// Returns value-free query action/source records.
    pub fn query(&self) -> &[EndpointQueryDiagnostic] {
        &self.query
    }
}

/// A fully validated endpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedEndpoint {
    url: Url,
    origin: Origin,
    diagnostics: EndpointResolutionDiagnostics,
}

impl ResolvedEndpoint {
    pub(crate) fn new(
        url: Url,
        diagnostics: EndpointResolutionDiagnostics,
    ) -> Result<Self, LlmError> {
        let origin = Origin::from_url(&url)?;
        Ok(Self {
            url,
            origin,
            diagnostics,
        })
    }

    /// Returns the final URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Returns the final origin.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Returns value-free endpoint resolution diagnostics.
    pub fn diagnostics(&self) -> &EndpointResolutionDiagnostics {
        &self.diagnostics
    }
}

impl fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field("origin", &self.origin)
            .field("query_names", &self.diagnostics.query)
            .field("path_variables", &self.diagnostics.path_variables)
            .finish_non_exhaustive()
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
