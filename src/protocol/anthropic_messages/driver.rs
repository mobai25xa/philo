#![allow(clippy::trivially_copy_pass_by_ref, clippy::unused_self)]

use http::Method;

use crate::error::LlmError;
use crate::plan::ResolvedCallPlan;
use crate::protocol::preparation::{
    CommonRequestFacts, ProtocolFactDecisions, request_facts, standard_json_sse_header_operations,
};
use crate::protocol::{
    AnthropicMessagesResponsePlan, HttpResponseRequirements, MaxOutputTokensSource, PreparedCall,
    ProtocolOperation, ProtocolRequestParts, ProtocolResponsePlan, ResponsePlan,
};

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
        let common = CommonRequestFacts::scan(&plan.planned);
        let reasoning_enabled = common.reasoning_requested()
            || plan
                .planned
                .options
                .protocol_options()
                .and_then(crate::protocol_options::ProtocolOptions::anthropic_messages)
                .is_some_and(|options| options.adaptive_thinking().is_some());
        let max_output_tokens_source = if common.max_output_tokens_requested() {
            MaxOutputTokensSource::Request
        } else {
            MaxOutputTokensSource::ModelDefault
        };
        Ok(PreparedCall {
            target: plan.policy.target.clone(),
            request: ProtocolRequestParts {
                method: Method::POST,
                operation: ProtocolOperation::Messages,
                protocol_headers: standard_json_sse_header_operations(),
                body,
            },
            response: ResponsePlan {
                http: HttpResponseRequirements::event_stream(
                    plan.policy.limits.transport.max_http_error_body_bytes,
                ),
                protocol: ProtocolResponsePlan::AnthropicMessages(AnthropicMessagesResponsePlan {
                    source: plan.planned.source.clone(),
                    response_format: plan.policy.response_format.clone(),
                    contract: *contract,
                    limits: plan.policy.limits.response,
                    sse: plan.policy.limits.transport.sse,
                }),
            },
            facts: request_facts(
                common,
                ProtocolFactDecisions {
                    reasoning_enabled,
                    max_output_tokens_source,
                },
            ),
            execution: plan.execution.clone(),
        })
    }
}
