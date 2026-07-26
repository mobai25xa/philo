//! Immutable provider policy captured for one logical call.
#![allow(dead_code)]
#![allow(clippy::struct_field_names)]

use std::fmt;

use crate::domain::{
    DialectPolicy, HistoryPolicy, ModelId, ProtocolId, ProviderId, RequestValidationLimits,
    ResourceLimits, ResponseFormat,
};
use crate::error::{ValidationError, ValidationReason};
use crate::transport::SseConfig;

use super::ProviderCapabilities;
use super::catalog::{DeploymentId, ProductId, ProviderModelId};
use super::compat::CompatProfile;
use super::compat::ResolvedProviderRouting;

/// Provider and protocol policy resolved before request preparation.
#[derive(Clone)]
pub(crate) struct CallPolicySnapshot {
    pub(crate) target: ResolvedTarget,
    pub(crate) capabilities: ProviderCapabilities,
    pub(crate) compat: ResolvedCompat,
    pub(crate) history: HistoryPolicy,
    pub(crate) limits: ResolvedLimits,
    pub(crate) response_format: ResponseFormat,
    pub(crate) provider_routing: Option<ResolvedProviderRouting>,
}

impl fmt::Debug for CallPolicySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallPolicySnapshot")
            .field("target", &self.target)
            .field("capabilities", &self.capabilities)
            .field("compat", &self.compat)
            .field("history", &self.history)
            .field("limits", &self.limits)
            .field(
                "response_format",
                &response_format_name(&self.response_format),
            )
            .field("has_provider_routing", &self.provider_routing.is_some())
            .finish()
    }
}

/// Concrete provider, protocol, and model identities selected for one call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedTarget {
    pub(crate) provider_id: ProviderId,
    pub(crate) product_id: ProductId,
    pub(crate) protocol_id: ProtocolId,
    pub(crate) protocol_kind: ProtocolKind,
    pub(crate) domain_model: ModelId,
    pub(crate) provider_model: ProviderModelId,
    pub(crate) deployment_id: Option<DeploymentId>,
    pub(crate) wire_model: ModelId,
}

/// Closed set of protocol implementations admitted by the current runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProtocolKind {
    OpenAiChatCompletions,
    AnthropicMessages,
}

/// Protocol compatibility policy compiled from the provider dialect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedCompat {
    pub(crate) dialect: DialectPolicy,
    pub(crate) profile: Option<CompatProfile>,
}

/// Complete request, response, and transport limits for one logical call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedLimits {
    pub(crate) request: RequestLimits,
    pub(crate) response: ResponseLimits,
    pub(crate) transport: TransportLimits,
    pub(crate) model: ModelPlanLimits,
}

impl ResolvedLimits {
    /// Compiles the single resource-limit snapshot consumed by later layers.
    pub(crate) fn compile(
        resources: ResourceLimits,
        sse: SseConfig,
        max_http_error_body_bytes: usize,
        max_output_tokens: Option<u32>,
        default_max_output_tokens: Option<u32>,
    ) -> Result<Self, ValidationError> {
        resources.validate()?;
        if max_http_error_body_bytes == 0 {
            return Err(ValidationError::new(
                "max_http_error_body_bytes",
                ValidationReason::Zero,
                "HTTP error body limit must be positive",
            ));
        }
        Ok(Self {
            request: RequestLimits {
                max_body_bytes: resources.max_request_body_bytes,
                max_messages: resources.max_messages,
                max_text_bytes: resources.max_total_text_bytes,
                max_tools: resources.max_tools,
                max_tool_description_bytes: resources.max_tool_description_bytes,
                max_schema_bytes: resources.max_schema_bytes,
                max_schema_depth: resources.max_schema_depth,
                max_json_array_items: resources.max_json_array_items,
                max_images: resources.max_images,
                max_inline_image_bytes: resources.max_inline_image_bytes,
                max_image_url_bytes: resources.max_image_url_bytes,
            },
            response: ResponseLimits {
                max_structured_output_bytes: resources.max_structured_output_bytes,
                max_tool_calls: resources.max_tool_calls,
                max_tool_arguments_bytes: resources.max_tool_arguments_bytes,
                max_all_tool_arguments_bytes: resources.max_all_tool_arguments_bytes,
                max_schema_depth: resources.max_schema_depth,
                max_json_array_items: resources.max_json_array_items,
            },
            transport: TransportLimits {
                max_http_error_body_bytes,
                sse,
            },
            model: ModelPlanLimits {
                max_output_tokens,
                default_max_output_tokens,
            },
        })
    }
}

/// Token ceilings that cannot be represented by byte-oriented resource limits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ModelPlanLimits {
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) default_max_output_tokens: Option<u32>,
}

/// Request-side ceilings consumed by planning and wire encoding.
pub(crate) type RequestLimits = RequestValidationLimits;

/// Response-side ceilings consumed by protocol state machines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResponseLimits {
    pub(crate) max_structured_output_bytes: usize,
    pub(crate) max_tool_calls: usize,
    pub(crate) max_tool_arguments_bytes: usize,
    pub(crate) max_all_tool_arguments_bytes: usize,
    pub(crate) max_schema_depth: usize,
    pub(crate) max_json_array_items: usize,
}

impl From<ResourceLimits> for ResponseLimits {
    fn from(resources: ResourceLimits) -> Self {
        Self {
            max_structured_output_bytes: resources.max_structured_output_bytes,
            max_tool_calls: resources.max_tool_calls,
            max_tool_arguments_bytes: resources.max_tool_arguments_bytes,
            max_all_tool_arguments_bytes: resources.max_all_tool_arguments_bytes,
            max_schema_depth: resources.max_schema_depth,
            max_json_array_items: resources.max_json_array_items,
        }
    }
}

/// HTTP and SSE ceilings consumed by one network attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransportLimits {
    pub(crate) max_http_error_body_bytes: usize,
    pub(crate) sse: SseConfig,
}

fn response_format_name(response_format: &ResponseFormat) -> &'static str {
    match response_format {
        ResponseFormat::Text => "text",
        ResponseFormat::JsonObject => "json_object",
        ResponseFormat::JsonSchema(_) => "json_schema",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CallPolicySnapshot, ResolvedCompat, ResolvedLimits, ResolvedTarget};
    use crate::domain::{
        DialectPolicy, HistoryPolicy, ModelId, ProtocolId, ProviderId, ResourceLimits,
        ResponseFormat, StructuredSchema, ToolSchema,
    };
    use crate::provider::{ProductId, ProviderCapabilities, ProviderModelId};
    use crate::transport::SseConfig;

    const CANARY: &str = "schema-canary-secret";

    #[test]
    fn resolved_limits_preserve_each_measurement_domain() {
        let resources = ResourceLimits::builder()
            .with_max_request_body_bytes(100)
            .with_max_structured_output_bytes(200)
            .build()
            .unwrap();
        let limits =
            ResolvedLimits::compile(resources, SseConfig::default(), 300, None, None).unwrap();
        assert_eq!(limits.request.max_body_bytes, 100);
        assert_eq!(limits.response.max_structured_output_bytes, 200);
        assert_eq!(limits.transport.max_http_error_body_bytes, 300);
    }

    #[test]
    fn policy_debug_does_not_expose_structured_schema() {
        let schema = ToolSchema::new(json!({
            "type": "object",
            "description": CANARY,
            "properties": {}
        }))
        .unwrap();
        let response_format = ResponseFormat::JsonSchema(
            StructuredSchema::new("safe_name", None, schema, false).unwrap(),
        );
        let policy = CallPolicySnapshot {
            target: ResolvedTarget {
                provider_id: ProviderId::new("official-openai").unwrap(),
                product_id: ProductId::new("chat-completions").unwrap(),
                protocol_id: ProtocolId::new("openai-chat").unwrap(),
                protocol_kind: super::ProtocolKind::OpenAiChatCompletions,
                domain_model: ModelId::new("gpt-test").unwrap(),
                provider_model: ProviderModelId::new("gpt-test").unwrap(),
                deployment_id: None,
                wire_model: ModelId::new("gpt-test").unwrap(),
            },
            capabilities: ProviderCapabilities::official_openai(),
            compat: ResolvedCompat {
                dialect: DialectPolicy::official_openai(),
                profile: Some(crate::provider::CompatProfile::openai_chat_default()),
            },
            history: HistoryPolicy::official_openai(),
            limits: ResolvedLimits::compile(
                ResourceLimits::official(),
                SseConfig::default(),
                16 * 1024,
                None,
                None,
            )
            .unwrap(),
            response_format,
            provider_routing: None,
        };
        let debug = format!("{policy:?}");
        assert!(debug.contains("json_schema"));
        assert!(!debug.contains(CANARY));
    }
}
