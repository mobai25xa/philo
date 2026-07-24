use std::fs;
use std::path::Path;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    GenerateRequest, GenerationOptions, LlmClient, Message, ModelRef, ToolDefinition, ToolName,
    ToolSchema,
};
use serde_json::json;

use super::case::ConformanceCase;
use super::redaction::{RedactedFailure, contains_forbidden_value};
use super::report::{CaseResult, CaseStatus};

const CREDENTIAL_CANARY: &str = "conformance-credential-canary";
const BODY_CANARY: &str = "conformance-body-canary";

/// Shared offline sections executed for every descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfflineSection {
    Descriptor,
    FixtureManifest,
    RuntimeBuild,
    EndpointAuthHeader,
    MinimalText,
    IllegalCapability,
    DiagnosticsRedaction,
}

impl OfflineSection {
    pub const ALL: [Self; 7] = [
        Self::Descriptor,
        Self::FixtureManifest,
        Self::RuntimeBuild,
        Self::EndpointAuthHeader,
        Self::MinimalText,
        Self::IllegalCapability,
        Self::DiagnosticsRedaction,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Descriptor => "descriptor",
            Self::FixtureManifest => "fixture_manifest",
            Self::RuntimeBuild => "runtime_build",
            Self::EndpointAuthHeader => "endpoint_auth_header",
            Self::MinimalText => "minimal_text",
            Self::IllegalCapability => "illegal_capability",
            Self::DiagnosticsRedaction => "diagnostics_redaction",
        }
    }
}

pub async fn run_offline(descriptor: &ConformanceCase) -> Result<Vec<CaseResult>, String> {
    let mut results = Vec::new();
    for section in OfflineSection::ALL {
        let outcome = run_section(descriptor, section).await;
        results.push(CaseResult {
            name: section.as_str().to_owned(),
            status: if outcome.is_ok() {
                CaseStatus::Passed
            } else {
                CaseStatus::Failed
            },
            reason_code: outcome.as_ref().err().copied(),
        });
        outcome.map_err(str::to_owned)?;
    }
    Ok(results)
}

async fn run_section(
    descriptor: &ConformanceCase,
    section: OfflineSection,
) -> Result<(), &'static str> {
    match section {
        OfflineSection::Descriptor => validate_descriptor(descriptor),
        OfflineSection::FixtureManifest => validate_fixture_manifest(descriptor),
        OfflineSection::RuntimeBuild => {
            let runtime = descriptor.profile.build(CREDENTIAL_CANARY);
            if runtime.provider_id().as_str() == descriptor.provider
                && runtime.product_id().as_str() == descriptor.product
            {
                Ok(())
            } else {
                Err("runtime_identity_mismatch")
            }
        }
        OfflineSection::EndpointAuthHeader | OfflineSection::MinimalText => {
            run_minimal(descriptor).await
        }
        OfflineSection::IllegalCapability => run_illegal_capability(descriptor).await,
        OfflineSection::DiagnosticsRedaction => validate_redaction(),
    }
}

fn validate_descriptor(descriptor: &ConformanceCase) -> Result<(), &'static str> {
    let required = [
        descriptor.id,
        descriptor.workflow_id,
        descriptor.provider,
        descriptor.product,
        descriptor.exact_model,
        descriptor.profile_version,
        descriptor.catalog_version,
        descriptor.compat_version,
        descriptor.endpoint_shape,
        descriptor.expected_endpoint,
        descriptor.auth_shape,
        descriptor.header_shape,
        descriptor.fixture_manifest,
        descriptor.source_kind,
        descriptor.reviewed_at,
        descriptor.evidence_expires_at,
    ];
    if required.iter().any(|value| value.trim().is_empty())
        || descriptor.capabilities.len() != super::case::OnlineCase::ALL.len()
        || descriptor.online.len() != super::case::OnlineCase::ALL.len()
    {
        Err("descriptor_incomplete")
    } else {
        Ok(())
    }
}

fn validate_fixture_manifest(descriptor: &ConformanceCase) -> Result<(), &'static str> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(descriptor.fixture_manifest);
    let text = fs::read_to_string(path).map_err(|_| "fixture_manifest_missing")?;
    let value: toml::Value = toml::from_str(&text).map_err(|_| "fixture_manifest_invalid")?;
    if value["provider"].as_str() == Some(descriptor.provider)
        && value["product"].as_str() == Some(descriptor.product)
        && value["source"].as_str().is_some()
        && value["reviewed_at"].as_str().is_some()
        && value["synthetic_conformance_claim"].as_bool() == Some(false)
    {
        Ok(())
    } else {
        Err("fixture_manifest_incomplete")
    }
}

async fn run_minimal(descriptor: &ConformanceCase) -> Result<(), &'static str> {
    let runtime = descriptor.profile.build(CREDENTIAL_CANARY);
    let transport = MockTransport::scripted([MockExchange::response(success())]);
    LlmClient::new(runtime, transport.clone())
        .complete(request(descriptor))
        .await
        .map_err(|_| "minimal_text_failed")?;
    let captured = transport.captured_requests();
    let request = captured.first().ok_or("transport_not_called")?;
    if request.endpoint().url().query().is_some()
        || request.endpoint().url().as_str() != descriptor.expected_endpoint
        || request.headers()[header::AUTHORIZATION]
            .to_str()
            .map_err(|_| "auth_header_invalid")?
            != format!("Bearer {CREDENTIAL_CANARY}")
        || request.headers()[header::CONTENT_TYPE] != "application/json"
    {
        return Err("request_shape_mismatch");
    }
    for (name, value) in descriptor.expected_headers {
        if request
            .headers()
            .get(*name)
            .and_then(|value| value.to_str().ok())
            != Some(*value)
        {
            return Err("provider_header_mismatch");
        }
    }
    Ok(())
}

async fn run_illegal_capability(descriptor: &ConformanceCase) -> Result<(), &'static str> {
    let runtime = descriptor.profile.build(CREDENTIAL_CANARY);
    let transport = MockTransport::default();
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false
    }))
    .map_err(|_| "tool_schema_invalid")?;
    let options = GenerationOptions::new().with_tools(vec![ToolDefinition::new(
        ToolName::new("conformance_tool").map_err(|_| "tool_name_invalid")?,
        schema,
    )]);
    let illegal = request(descriptor).with_options(options);
    if LlmClient::new(runtime, transport.clone())
        .stream(illegal)
        .await
        .is_ok()
        || !transport.captured_requests().is_empty()
    {
        Err("illegal_capability_reached_transport")
    } else {
        Ok(())
    }
}

fn validate_redaction() -> Result<(), &'static str> {
    let failure = RedactedFailure::observe(
        "authentication",
        Some(401),
        Some("invalid_api_key"),
        BODY_CANARY.as_bytes(),
    );
    let encoded = serde_json::to_string(&failure).map_err(|_| "report_serialization_failed")?;
    if contains_forbidden_value(&encoded, &[BODY_CANARY, CREDENTIAL_CANARY])
        || failure.body_length != BODY_CANARY.len()
    {
        Err("redaction_canary_leaked")
    } else {
        Ok(())
    }
}

fn request(descriptor: &ConformanceCase) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new(descriptor.provider, descriptor.exact_model).unwrap(),
        vec![Message::user("fixed minimal offline prompt")],
    )
}

fn success() -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let delta = json!({
        "id": "conformance-generation",
        "model": "conformance-model",
        "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": null}]
    });
    let finish = json!({
        "id": "conformance-generation",
        "model": "conformance-model",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from(format!(
            "data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
        )))],
    )
}
