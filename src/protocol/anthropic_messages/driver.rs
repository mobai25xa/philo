#![allow(clippy::trivially_copy_pass_by_ref, clippy::unused_self)]

use http::{HeaderValue, Method, header};

use crate::domain::{ContentPart, MessageRole, ThinkingRequest};
use crate::error::LlmError;
use crate::plan::ResolvedCallPlan;
use crate::protocol::{
    AnthropicMessagesResponsePlan, ExpectedContentType, HttpResponseRequirements,
    MaxOutputTokensSource, PreparedCall, ProtocolOperation, ProtocolRequestParts,
    ProtocolResponsePlan, RequestFacts, ResponseFormatKind, ResponsePlan,
};
use crate::provider::HeaderOperation;

use super::request::encode_planned_request;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AnthropicMessagesDriver;

impl AnthropicMessagesDriver {
    pub(crate) fn prepare(&self, plan: &ResolvedCallPlan) -> Result<PreparedCall, LlmError> {
        let contract = plan.policy.protocol.anthropic_messages().ok_or_else(|| {
            crate::error::ProtocolError::new(
                "Anthropic Messages driver requires an Anthropic Messages protocol contract",
            )
        })?;
        let body = encode_planned_request(plan)?;
        let protocol_headers = vec![
            HeaderOperation::set(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            HeaderOperation::set(
                header::ACCEPT,
                HeaderValue::from_static("text/event-stream"),
            ),
        ];
        let contains_images = plan.planned.messages.iter().any(|message| {
            message
                .content()
                .iter()
                .any(|part| matches!(part, ContentPart::Image(_)))
        });
        Ok(PreparedCall {
            target: plan.policy.target.clone(),
            request: ProtocolRequestParts {
                method: Method::POST,
                operation: ProtocolOperation::Messages,
                protocol_headers,
                body,
            },
            response: ResponsePlan {
                http: HttpResponseRequirements {
                    content_type: ExpectedContentType::EventStream,
                    max_error_body_bytes: plan.policy.limits.transport.max_http_error_body_bytes,
                },
                protocol: ProtocolResponsePlan::AnthropicMessages(AnthropicMessagesResponsePlan {
                    source: plan.planned.source.clone(),
                    response_format: plan.policy.response_format.clone(),
                    contract: *contract,
                    limits: plan.policy.limits.response,
                    sse: plan.policy.limits.transport.sse,
                }),
            },
            facts: RequestFacts {
                contains_tools: !plan.planned.options.tools().is_empty()
                    || plan.planned.messages.iter().any(|message| {
                        message.role() == MessageRole::Tool
                            || message
                                .content()
                                .iter()
                                .any(|part| matches!(part, ContentPart::ToolCall(_)))
                    }),
                contains_images,
                reasoning_enabled: !matches!(
                    plan.planned.options.reasoning(),
                    ThinkingRequest::ProviderDefault
                ) || plan
                    .planned
                    .options
                    .protocol_options()
                    .and_then(crate::protocol_options::ProtocolOptions::anthropic_messages)
                    .is_some_and(|options| options.adaptive_thinking().is_some()),
                response_format: ResponseFormatKind::from(plan.planned.options.response_format()),
                max_output_tokens_source: if plan.planned.options.max_output_tokens().is_some() {
                    MaxOutputTokensSource::Request
                } else {
                    MaxOutputTokensSource::ModelDefault
                },
            },
            execution: plan.execution.clone(),
        })
    }
}
