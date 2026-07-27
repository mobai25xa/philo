//! Cross-field validation and compilation into core construction inputs.
#![allow(clippy::missing_errors_doc)]

use philo::domain::{ProtocolId, ProviderId};
use philo::error::{LlmError, ProviderConfigError, ProviderConfigFailure};
use philo::provider::auth::ClientIdentity;
use philo::provider::endpoint::EndpointConfig;
use philo::provider::profiles::{OfficialAnthropicProfile, OfficialOpenAiProfile};
use philo::provider::runtime::ProviderRuntime;
use philo::provider::secret::SecretResolver;
use philo::provider::{ProviderDefinition, ProviderDeploymentConfig};

use super::merge::ProviderConfigSnapshot;
use super::schema::{CredentialAudienceSpec, EndpointSpec};

pub(crate) fn validate_snapshot(
    snapshot: &ProviderConfigSnapshot,
) -> Result<(), ProviderConfigError> {
    snapshot.version().validate()?;
    required(snapshot.provider_id_field(), "provider_id")?;
    required(snapshot.protocol_id_field(), "protocol_id")?;
    required(snapshot.endpoint_field(), "endpoint")?;
    required(snapshot.audience_field(), "credential_audience")?;
    required(snapshot.identity_field(), "client_identity")?;
    ProviderId::new(required(snapshot.provider_id_field(), "provider_id")?.clone()).map_err(
        |_| {
            with_source(
                ProviderConfigError::new(
                    "provider_id",
                    ProviderConfigFailure::InvalidValue,
                    "provider identifier is invalid",
                ),
                snapshot.provider_id_field().source_id(),
            )
        },
    )?;
    ProtocolId::new(required(snapshot.protocol_id_field(), "protocol_id")?.clone()).map_err(
        |_| {
            with_source(
                ProviderConfigError::new(
                    "protocol_id",
                    ProviderConfigFailure::InvalidValue,
                    "protocol identifier is invalid",
                ),
                snapshot.protocol_id_field().source_id(),
            )
        },
    )?;
    let limit = required(snapshot.error_limit_field(), "max_http_error_body_bytes")?;
    if *limit == 0 || *limit > 1024 * 1024 {
        return invalid(
            snapshot.error_limit_field().source_id(),
            "max_http_error_body_bytes",
            "HTTP error body limit must be between 1 byte and 1 MiB",
        );
    }
    if let Some(endpoint) = snapshot.endpoint_field().value() {
        compile_endpoint(endpoint).map_err(|_| {
            with_source(
                ProviderConfigError::new(
                    "endpoint",
                    ProviderConfigFailure::InvalidValue,
                    "endpoint configuration is invalid",
                ),
                snapshot.endpoint_field().source_id(),
            )
        })?;
    }
    if let Some(identity) = snapshot.identity_field().value() {
        ClientIdentity::new(identity.product.clone(), identity.version.clone()).map_err(|_| {
            with_source(
                ProviderConfigError::new(
                    "client_identity",
                    ProviderConfigFailure::InvalidValue,
                    "client identity configuration is invalid",
                ),
                snapshot.identity_field().source_id(),
            )
        })?;
    }
    if let Some(reference) = snapshot.credential_field().value() {
        reference
            .validate()
            .map_err(|error| with_source(error, snapshot.credential_field().source_id()))?;
    }
    Ok(())
}

impl ProviderConfigSnapshot {
    /// Compiles this snapshot into the core's two construction inputs.
    ///
    /// This is the entire contract between a configuration document and the
    /// SDK: a secret-free [`ProviderDefinition`] and a
    /// [`ProviderDeploymentConfig`] that names — but never carries — the
    /// credential. The core compiles those; it has no second entry point that
    /// understands configuration documents.
    pub fn official_openai_inputs(
        &self,
    ) -> Result<(ProviderDefinition, ProviderDeploymentConfig), LlmError> {
        validate_snapshot(self)?;
        require_exact(self.provider_id_field(), "official-openai", "provider_id")?;
        require_exact(
            self.protocol_id_field(),
            "openai-chat-completions",
            "protocol_id",
        )?;
        self.require_audience(CredentialAudienceSpec::OfficialOpenAi)?;
        self.require_endpoint(&EndpointConfig::base_and_path(
            "https://api.openai.com/v1",
            "/chat/completions",
        )?)?;
        let definition = OfficialOpenAiProfile::definition()?;
        let deployment = self.deployment_for(definition.provider_id().clone())?;
        Ok((definition, deployment))
    }

    /// Compiles and freezes the official `OpenAI` runtime.
    pub fn build_official_openai_runtime<R: SecretResolver + ?Sized>(
        &self,
        resolver: &R,
    ) -> Result<ProviderRuntime, LlmError> {
        let (definition, deployment) = self.official_openai_inputs()?;
        ProviderRuntime::build(definition.compile(&deployment, resolver)?)
    }

    /// Compiles this snapshot into the official Anthropic construction inputs.
    pub fn official_anthropic_inputs(
        &self,
    ) -> Result<(ProviderDefinition, ProviderDeploymentConfig), LlmError> {
        validate_snapshot(self)?;
        require_exact(
            self.provider_id_field(),
            "official-anthropic",
            "provider_id",
        )?;
        require_exact(
            self.protocol_id_field(),
            "anthropic-messages",
            "protocol_id",
        )?;
        self.require_audience(CredentialAudienceSpec::OfficialAnthropic)?;
        self.require_endpoint(&EndpointConfig::base_and_path(
            "https://api.anthropic.com/v1",
            "/messages",
        )?)?;
        let definition = OfficialAnthropicProfile::definition()?;
        let deployment = self.deployment_for(definition.provider_id().clone())?;
        Ok((definition, deployment))
    }

    /// Compiles and freezes the official Anthropic Messages runtime.
    pub fn build_official_anthropic_runtime<R: SecretResolver + ?Sized>(
        &self,
        resolver: &R,
    ) -> Result<ProviderRuntime, LlmError> {
        let (definition, deployment) = self.official_anthropic_inputs()?;
        ProviderRuntime::build(definition.compile(&deployment, resolver)?)
    }

    fn require_audience(
        &self,
        expected: CredentialAudienceSpec,
    ) -> Result<(), ProviderConfigError> {
        if self.credential_audience() == Some(expected) {
            return Ok(());
        }
        Err(with_source(
            ProviderConfigError::new(
                "credential_audience",
                ProviderConfigFailure::ForbiddenOverride,
                "official profile requires the official credential audience",
            ),
            self.audience_field().source_id(),
        ))
    }

    fn require_endpoint(&self, expected: &EndpointConfig) -> Result<(), LlmError> {
        let actual = compile_endpoint(required(self.endpoint_field(), "endpoint")?)?;
        if &actual == expected {
            return Ok(());
        }
        Err(with_source(
            ProviderConfigError::new(
                "endpoint",
                ProviderConfigFailure::ForbiddenOverride,
                "official profile endpoint cannot be overridden",
            ),
            self.endpoint_field().source_id(),
        )
        .into())
    }

    fn deployment_for(
        &self,
        provider_id: ProviderId,
    ) -> Result<ProviderDeploymentConfig, LlmError> {
        let reference = required(self.credential_field(), "credential")?.clone();
        let identity = required(self.identity_field(), "client_identity")?;
        let identity = ClientIdentity::new(identity.product.clone(), identity.version.clone())?;
        let limit = *required(self.error_limit_field(), "max_http_error_body_bytes")?;
        ProviderDeploymentConfig::new(provider_id, reference)
            .with_client_identity(identity)
            .with_max_http_error_body_bytes(limit)
    }
}

fn compile_endpoint(spec: &EndpointSpec) -> Result<EndpointConfig, LlmError> {
    match spec {
        EndpointSpec::BaseAndPath {
            base_url,
            endpoint_path,
        } => {
            if base_url.len() > 2048 || endpoint_path.len() > 1024 {
                return Err(LlmError::Configuration(
                    "endpoint configuration exceeds bounded lengths".to_owned(),
                ));
            }
            EndpointConfig::base_and_path(base_url, endpoint_path.clone())
        }
        EndpointSpec::Absolute { url } => {
            if url.len() > 3072 {
                return Err(LlmError::Configuration(
                    "endpoint configuration exceeds bounded lengths".to_owned(),
                ));
            }
            EndpointConfig::absolute(url)
        }
    }
}

fn require_exact(
    field: &super::merge::ResolvedField<String>,
    expected: &str,
    name: &'static str,
) -> Result<(), ProviderConfigError> {
    if required(field, name)? == expected {
        Ok(())
    } else {
        Err(with_source(
            ProviderConfigError::new(
                name,
                ProviderConfigFailure::ForbiddenOverride,
                "built-in profile identity cannot be overridden",
            ),
            field.source_id(),
        ))
    }
}

fn required<'a, T>(
    field: &'a super::merge::ResolvedField<T>,
    name: &'static str,
) -> Result<&'a T, ProviderConfigError> {
    field.value().ok_or_else(|| {
        with_source(
            ProviderConfigError::new(
                name,
                ProviderConfigFailure::MissingRequiredField,
                "required provider configuration field is absent",
            ),
            field.source_id(),
        )
    })
}

fn invalid<T>(
    source: Option<&str>,
    field: &'static str,
    message: &'static str,
) -> Result<T, ProviderConfigError> {
    Err(with_source(
        ProviderConfigError::new(field, ProviderConfigFailure::InvalidValue, message),
        source,
    ))
}

fn with_source(error: ProviderConfigError, source: Option<&str>) -> ProviderConfigError {
    match source {
        Some(source) => error.with_source(source),
        None => error,
    }
}
