//! Concrete `OpenAI` Chat Completions request/response driver.
#![allow(clippy::trivially_copy_pass_by_ref, clippy::unused_self)]

use http::Method;

use crate::error::LlmError;
use crate::plan::ResolvedCallPlan;

use super::request::encode_planned_request;
use crate::protocol::preparation::{
    CommonRequestFacts, ProtocolFactDecisions, request_facts, standard_json_sse_header_operations,
};
use crate::protocol::{
    HttpResponseRequirements, MaxOutputTokensSource, OpenAiChatResponsePlan, PreparedCall,
    ProtocolOperation, ProtocolRequestParts, ProtocolResponsePlan, ResponsePlan,
};

/// Stateless `OpenAI` Chat Completions protocol implementation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpenAiChatDriver;

impl OpenAiChatDriver {
    /// Converts a fully planned logical call into owned protocol request parts.
    pub(crate) fn prepare(&self, plan: &ResolvedCallPlan) -> Result<PreparedCall, LlmError> {
        let contract = plan.policy.protocol.openai_chat().ok_or_else(|| {
            crate::error::ProtocolError::new(
                "OpenAI Chat driver requires an OpenAI Chat protocol contract",
            )
        })?;
        let body = encode_planned_request(plan)?;
        let common = CommonRequestFacts::scan(&plan.planned);
        let max_output_tokens_source = if common.max_output_tokens_requested() {
            MaxOutputTokensSource::Request
        } else if plan.policy.limits.model.default_max_output_tokens.is_some() {
            MaxOutputTokensSource::ModelDefault
        } else {
            MaxOutputTokensSource::Omitted
        };

        Ok(PreparedCall {
            target: plan.policy.target.clone(),
            request: ProtocolRequestParts {
                method: Method::POST,
                operation: ProtocolOperation::ChatCompletions,
                protocol_headers: standard_json_sse_header_operations(),
                body,
            },
            response: ResponsePlan {
                http: HttpResponseRequirements::event_stream(
                    plan.policy.limits.transport.max_http_error_body_bytes,
                ),
                protocol: ProtocolResponsePlan::OpenAiChat(OpenAiChatResponsePlan {
                    model: plan.planned.model.clone(),
                    response_format: plan.policy.response_format.clone(),
                    contract: contract.clone(),
                    limits: plan.policy.limits.response,
                    sse: plan.policy.limits.transport.sse,
                }),
            },
            facts: request_facts(
                common,
                ProtocolFactDecisions {
                    reasoning_enabled: common.reasoning_requested(),
                    max_output_tokens_source,
                },
            ),
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

    fn assert_static<T: 'static>(_: &T) {}

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
        drop(plan);
        drop(request);
        assert_static(&prepared);
        let body: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(prepared.request.protocol_headers.len(), 2);
        assert_eq!(
            prepared.request.operation,
            crate::protocol::ProtocolOperation::ChatCompletions
        );
        assert_eq!(
            prepared.response.http.content_type,
            crate::protocol::ExpectedContentType::EventStream
        );
        assert!(!prepared.facts.contains_tools);
        assert!(!prepared.facts.contains_images);
        assert!(!prepared.facts.reasoning_enabled);
        assert_eq!(
            prepared.facts.max_output_tokens_source,
            crate::protocol::MaxOutputTokensSource::Omitted
        );
    }

    #[test]
    fn driver_rejects_another_protocol_contract() {
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/chat/completions", "test-key")
                .unwrap()
                .build()
                .unwrap();
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("hello")],
        );
        let mut plan = CallPlanner::plan(&runtime, &request).unwrap();
        plan.policy.protocol =
            crate::provider::ResolvedProtocolContract::strict_anthropic_messages();
        assert!(OpenAiChatDriver.prepare(&plan).is_err());
    }
}
