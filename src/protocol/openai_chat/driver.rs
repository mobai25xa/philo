//! Concrete `OpenAI` Chat Completions request/response driver.
#![allow(clippy::trivially_copy_pass_by_ref, clippy::unused_self)]

use http::{HeaderValue, Method, header};

use crate::domain::{ContentPart, MessageRole, ThinkingRequest};
use crate::error::LlmError;
use crate::execution::contract::ResolvedCallPlan;
use crate::provider::HeaderOperation;

use super::request::encode_planned_request;
use crate::protocol::{
    ExpectedContentType, HttpResponseRequirements, MaxOutputTokensSource, OpenAiChatResponsePlan,
    PreparedCall, ProtocolOperation, ProtocolRequestParts, ProtocolResponsePlan, RequestFacts,
    ResponseFormatKind, ResponsePlan,
};

/// Stateless `OpenAI` Chat Completions protocol implementation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpenAiChatDriver;

impl OpenAiChatDriver {
    /// Converts a fully planned logical call into owned protocol request parts.
    pub(crate) fn prepare(&self, plan: &ResolvedCallPlan) -> Result<PreparedCall, LlmError> {
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
                operation: ProtocolOperation::ChatCompletions,
                protocol_headers,
                body,
            },
            response: ResponsePlan {
                http: HttpResponseRequirements {
                    content_type: ExpectedContentType::EventStream,
                    max_error_body_bytes: plan.policy.limits.transport.max_http_error_body_bytes,
                },
                protocol: ProtocolResponsePlan::OpenAiChat(OpenAiChatResponsePlan {
                    model: plan.planned.model.clone(),
                    response_format: plan.policy.response_format.clone(),
                    compat: plan.policy.compat.clone(),
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
                ),
                response_format: ResponseFormatKind::from(plan.planned.options.response_format()),
                max_output_tokens_source: if plan.planned.options.max_output_tokens().is_some() {
                    MaxOutputTokensSource::Request
                } else if plan.policy.limits.model.default_max_output_tokens.is_some() {
                    MaxOutputTokensSource::ModelDefault
                } else {
                    MaxOutputTokensSource::Omitted
                },
            },
            execution: plan.execution.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::domain::{GenerateRequest, Message, ModelRef};
    use crate::execution::planner::CallPlanner;
    use crate::provider::TestOnlyProfile;

    use super::OpenAiChatDriver;

    #[test]
    fn prepare_emits_owned_openai_request_parts() {
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/chat/completions", "test-key")
                .unwrap()
                .build()
                .unwrap();
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("hello")],
        );
        let plan = CallPlanner::plan(&runtime, &request).unwrap();
        let prepared = OpenAiChatDriver.prepare(&plan).unwrap();
        let body: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(prepared.request.protocol_headers.len(), 2);
    }
}
