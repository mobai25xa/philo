//! Deterministic compilation of a logical generation request.

use crate::domain::{
    GenerateRequest, HistoryCapabilities, normalize_history_with_limits, validate_planned_request,
    validate_request_shape,
};
use crate::error::LlmError;
use crate::provider::ProviderRuntime;

use crate::plan::{
    CallExecutionIntent, NormalizationReport, PlanProvenance, PlannedRequest, ResolvedCallPlan,
};

/// Sole production owner of policy resolution and history normalization.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CallPlanner;

impl CallPlanner {
    /// Compiles one immutable, fully validated logical-call plan without I/O.
    pub(crate) fn plan(
        runtime: &ProviderRuntime,
        request: &GenerateRequest,
    ) -> Result<ResolvedCallPlan, LlmError> {
        let policy = runtime.plan_policy_for(request)?;
        let (capability_source, model_override_applied) =
            runtime.policy_provenance_for(request.model().model());
        let capabilities = policy.capabilities.generation_options();
        if let (Some(requested), Some(maximum)) = (
            request.options().max_output_tokens(),
            policy.limits.model.max_output_tokens,
        ) && requested > maximum
        {
            return Err(crate::error::ValidationError::new(
                "max_output_tokens",
                crate::error::ValidationReason::OutOfRange,
                "max_output_tokens exceeds the exact model catalog limit",
            )
            .into());
        }
        validate_request_shape(request, &capabilities, &policy.limits.request)?;

        let input_message_count = request.messages().len();
        let normalized = normalize_history_with_limits(
            request.messages(),
            &HistoryCapabilities::new(
                policy.capabilities.developer_role,
                policy.capabilities.vision_input,
            ),
            &policy.protocol.dialect_policy(),
            &policy.history,
            policy.limits.request.max_messages,
            policy.limits.request.max_text_bytes,
        )?;
        let planned = PlannedRequest {
            model: request.model().clone(),
            source: crate::domain::SourceIdentity::new(
                policy.target.provider_id.clone(),
                request.model().model().clone(),
                policy.target.protocol_id.clone(),
            ),
            messages: normalized.messages().to_vec(),
            options: request.options().clone(),
            normalization: NormalizationReport {
                mappings: normalized.id_mappings().to_vec(),
                diagnostics: normalized.diagnostics().to_vec(),
                input_message_count,
                output_message_count: normalized.messages().len(),
            },
        };
        validate_planned_request(
            &planned.model,
            &planned.messages,
            &planned.options,
            &capabilities,
            &policy.limits.request,
        )?;

        Ok(ResolvedCallPlan {
            planned,
            provenance: PlanProvenance {
                capability_source,
                compat_source: policy.protocol.openai_chat().map_or(
                    crate::domain::PolicySource::ProtocolDefault,
                    |contract| {
                        contract
                            .compat()
                            .source(crate::provider::CompatField::RequestMaxOutputTokens)
                    },
                ),
                model_override_applied,
            },
            execution: CallExecutionIntent {
                request_headers: request.options().headers().clone(),
                timeout: request.options().timeout(),
            },
            policy,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        CapabilityStatus, ContentPart, DiagnosticCode, GenerateRequest, Message, MessageRole,
        ModelId, ModelRef, ResourceLimits, ToolArguments, ToolCall, ToolCallId, ToolName,
    };
    use crate::error::{HistoryFailure, LlmError, ValidationReason};
    use crate::provider::{ModelCapabilityProfile, TestOnlyProfile};

    use super::CallPlanner;

    fn runtime() -> crate::provider::ProviderRuntime {
        TestOnlyProfile::localhost("http://127.0.0.1:8787/chat/completions", "test-key")
            .unwrap()
            .with_model_capabilities(
                ModelCapabilityProfile::new(ModelId::new("gpt-test").unwrap())
                    .with_function_tools(CapabilityStatus::Supported)
                    .with_tool_choice_required(CapabilityStatus::Supported)
                    .with_tool_choice_specific(CapabilityStatus::Supported)
                    .with_parallel_tool_calls(CapabilityStatus::Supported)
                    .with_strict_tools(CapabilityStatus::Supported)
                    .with_vision_input(CapabilityStatus::Supported)
                    .with_image_detail_original(CapabilityStatus::Supported)
                    .with_response_format_json_object(CapabilityStatus::Supported)
                    .with_response_format_json_schema(CapabilityStatus::Supported),
            )
            .build()
            .unwrap()
    }

    fn model() -> ModelRef {
        ModelRef::new("test-only", "gpt-test").unwrap()
    }

    #[test]
    fn planning_removes_empty_assistant_without_mutating_input() {
        let request = GenerateRequest::new(
            model(),
            vec![
                Message::new(MessageRole::Assistant, Vec::new()),
                Message::user("hello"),
            ],
        );
        let plan = CallPlanner::plan(&runtime(), &request).unwrap();
        assert_eq!(request.messages().len(), 2);
        assert_eq!(plan.planned.messages.len(), 1);
        assert!(plan.planned.normalization.diagnostics.iter().any(|item| {
            item.code() == DiagnosticCode::RemovedEmptyAssistant && item.count() == 1
        }));
    }

    #[test]
    fn planning_is_deterministic() {
        let request = GenerateRequest::new(model(), vec![Message::user("hello")]);
        let first = CallPlanner::plan(&runtime(), &request).unwrap();
        let second = CallPlanner::plan(&runtime(), &request).unwrap();
        assert_eq!(first.planned.messages, second.planned.messages);
        assert_eq!(first.planned.normalization, second.planned.normalization);
        assert_eq!(first.policy.target, second.policy.target);
    }

    #[test]
    fn provider_mismatch_fails_before_any_protocol_preparation() {
        let request = GenerateRequest::new(
            ModelRef::new("other-provider", "gpt-test").unwrap(),
            vec![Message::user("hello")],
        );
        let error = CallPlanner::plan(&runtime(), &request).unwrap_err();
        assert!(matches!(
            error,
            LlmError::Validation(ref error)
                if error.reason() == ValidationReason::ProviderMismatch
        ));
    }

    #[test]
    fn missing_tool_result_is_owned_by_planning() {
        let call = ToolCall::new(
            ToolCallId::new("call_1").unwrap(),
            ToolName::new("lookup").unwrap(),
            ToolArguments::from_raw_json(r#"{"city":"Paris"}"#).unwrap(),
        );
        let request = GenerateRequest::new(
            model(),
            vec![
                Message::user("lookup"),
                Message::new(MessageRole::Assistant, vec![ContentPart::ToolCall(call)]),
                Message::user("continue"),
            ],
        );
        let error = CallPlanner::plan(&runtime(), &request).unwrap_err();
        assert!(matches!(
            error,
            LlmError::History(ref error)
                if error.reason() == HistoryFailure::MissingToolResult
        ));
    }

    #[test]
    fn resolved_history_resource_limits_reject_instead_of_truncating() {
        let limits = ResourceLimits::builder()
            .with_max_messages(1)
            .with_max_total_text_bytes(4)
            .build()
            .unwrap();
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/chat/completions", "test-key")
                .unwrap()
                .with_resource_limits(limits)
                .build()
                .unwrap();

        let too_many =
            GenerateRequest::new(model(), vec![Message::user("one"), Message::user("two")]);
        let error = CallPlanner::plan(&runtime, &too_many).unwrap_err();
        assert!(matches!(
            error,
            LlmError::Validation(ref error)
                if error.reason() == ValidationReason::OutOfRange
        ));

        let too_large = GenerateRequest::new(model(), vec![Message::user("12345")]);
        let error = CallPlanner::plan(&runtime, &too_large).unwrap_err();
        assert!(matches!(
            error,
            LlmError::History(ref error) if error.reason() == HistoryFailure::TextTooLarge
        ));
    }
}
