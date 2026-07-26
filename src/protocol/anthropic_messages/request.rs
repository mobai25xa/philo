use bytes::Bytes;

use crate::domain::{ParallelToolCalls, ResponseFormat, ThinkingRequest, ToolChoice};
use crate::error::{LlmError, ProtocolError, ValidationError, ValidationReason};
use crate::execution::contract::ResolvedCallPlan;
use crate::protocol_options::{AnthropicEffort, AnthropicThinkingDisplay};

use super::history::plan_history;
use super::wire::{
    AnthropicEffortWire, MessagesRequestWire, OutputConfigWire, OutputFormatWire,
    ThinkingConfigWire, ThinkingDisplayWire, ThinkingKindWire, ToolChoiceWire, ToolWire,
};

#[allow(clippy::too_many_lines)]
pub(super) fn encode_planned_request(plan: &ResolvedCallPlan) -> Result<Bytes, LlmError> {
    let anthropic_options = plan
        .planned
        .options
        .protocol_options()
        .and_then(crate::protocol_options::ProtocolOptions::anthropic_messages);
    let max_tokens = plan
        .planned
        .options
        .max_output_tokens()
        .or(plan.policy.limits.model.default_max_output_tokens)
        .ok_or_else(|| {
            ValidationError::new(
                "max_output_tokens",
                ValidationReason::Empty,
                "Anthropic Messages requires an explicit or profile default max output token limit",
            )
        })?;
    if max_tokens == 0 {
        return Err(ValidationError::new(
            "max_output_tokens",
            ValidationReason::Zero,
            "Anthropic Messages max_tokens must be positive",
        )
        .into());
    }
    let temperature = plan.planned.options.temperature();
    if temperature.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
        return Err(ValidationError::new(
            "temperature",
            ValidationReason::OutOfRange,
            "Anthropic Messages temperature must be between zero and one",
        )
        .into());
    }
    if !matches!(
        plan.planned.options.reasoning(),
        ThinkingRequest::ProviderDefault
    ) {
        return Err(ProtocolError::new(
            "common thinking intent requires Anthropic typed protocol options",
        )
        .into());
    }

    let history = plan_history(plan)?;
    let _history_diagnostic_count = history.diagnostics().len();

    let tools = (!plan.planned.options.tools().is_empty()).then(|| {
        plan.planned
            .options
            .tools()
            .iter()
            .map(|tool| ToolWire {
                name: tool.name().as_str().to_owned(),
                description: tool.description().map(str::to_owned),
                input_schema: tool.parameters().value().clone(),
                strict: tool.strict(),
            })
            .collect::<Vec<_>>()
    });
    let disable_parallel = matches!(
        plan.planned.options.parallel_tool_calls(),
        Some(ParallelToolCalls::Disabled)
    );
    let tool_choice = match plan.planned.options.tool_choice() {
        None => None,
        Some(ToolChoice::None) => Some(ToolChoiceWire::None {
            disable_parallel_tool_use: disable_parallel,
        }),
        Some(ToolChoice::Auto) => Some(ToolChoiceWire::Auto {
            disable_parallel_tool_use: disable_parallel,
        }),
        Some(ToolChoice::Required) => Some(ToolChoiceWire::Any {
            disable_parallel_tool_use: disable_parallel,
        }),
        Some(ToolChoice::Specific { name }) => Some(ToolChoiceWire::Tool {
            name: name.as_str().to_owned(),
            disable_parallel_tool_use: disable_parallel,
        }),
    };
    if tool_choice.is_some() && tools.is_none() {
        return Err(ProtocolError::new("Anthropic tool_choice requires declared tools").into());
    }

    let output_config = match plan.planned.options.response_format() {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => {
            return Err(ProtocolError::new(
                "generic JSON object mode is unsupported by Anthropic Messages",
            )
            .into());
        }
        ResponseFormat::JsonSchema(schema) => Some(OutputFormatWire::JsonSchema {
            schema: schema.schema().value().clone(),
        }),
    };

    let effort = anthropic_options
        .and_then(crate::extensions::AnthropicMessagesOptions::effort)
        .map(|effort| match effort {
            AnthropicEffort::Low => AnthropicEffortWire::Low,
            AnthropicEffort::Medium => AnthropicEffortWire::Medium,
            AnthropicEffort::High => AnthropicEffortWire::High,
            AnthropicEffort::Max => AnthropicEffortWire::Max,
        });
    let output_config = (output_config.is_some() || effort.is_some()).then_some(OutputConfigWire {
        format: output_config,
        effort,
    });
    let thinking = anthropic_options
        .and_then(crate::extensions::AnthropicMessagesOptions::adaptive_thinking)
        .map(|display| ThinkingConfigWire {
            kind: ThinkingKindWire::Adaptive,
            display: match display {
                AnthropicThinkingDisplay::Omitted => ThinkingDisplayWire::Omitted,
                AnthropicThinkingDisplay::Summarized => ThinkingDisplayWire::Summarized,
            },
        });

    let wire = MessagesRequestWire {
        model: plan.policy.target.wire_model.as_str().to_owned(),
        max_tokens,
        messages: history.messages,
        system: history.system,
        stream: true,
        temperature,
        tools,
        tool_choice,
        thinking,
        output_config,
    };
    let mut body_value = serde_json::to_value(&wire)
        .map_err(|_| ProtocolError::new("failed to serialize Anthropic Messages request"))?;
    if let Some(raw) = anthropic_options.and_then(|options| options.raw()) {
        let object = body_value.as_object_mut().ok_or_else(|| {
            ProtocolError::new("Anthropic Messages request did not serialize as an object")
        })?;
        for (name, value) in raw.fields() {
            if object.contains_key(name) {
                return Err(ValidationError::new(
                    "protocol_options.anthropic.raw",
                    ValidationReason::Conflict,
                    "raw extension conflicts with an SDK-owned request field",
                )
                .into());
            }
            object.insert(name.clone(), value.clone());
        }
    }
    let body = serde_json::to_vec(&body_value)
        .map_err(|_| ProtocolError::new("failed to serialize Anthropic Messages request"))?;
    if body.len() > plan.policy.limits.request.max_body_bytes {
        return Err(ValidationError::new(
            "request_body",
            ValidationReason::OutOfRange,
            "encoded Anthropic Messages request exceeds the resolved body limit",
        )
        .into());
    }
    Ok(Bytes::from(body))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use serde_json::Value;

    use crate::domain::{
        CapabilityStatus, ContentPart, GenerateRequest, GenerationOptions, ImageContent,
        ImageDetail, Message, MessageRole, ModelId, ModelRef, OpaqueReasoning, ParallelToolCalls,
        PolicySource, ProtocolId, ProviderId, ResponseFormat, SourceIdentity, StructuredSchema,
        ThinkingContent, ThinkingReplayPolicy, ToolArguments, ToolCall, ToolCallId, ToolChoice,
        ToolDefinition, ToolName, ToolResultMessage, ToolSchema,
    };
    use crate::execution::planner::CallPlanner;
    use crate::protocol::openai_chat::OpenAiChatDriver;
    use crate::provider::{CompatPatch, ModelCapabilityProfile, TestOnlyProfile};
    use crate::{
        AnthropicEffort, AnthropicMessagesOptions, AnthropicRawExtension, AnthropicThinkingDisplay,
    };

    use super::super::AnthropicMessagesDriver;

    fn plan(
        messages: Vec<Message>,
        options: GenerationOptions,
    ) -> crate::execution::contract::ResolvedCallPlan {
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/v1/messages", "request-test-key")
                .unwrap()
                .with_model_capabilities(
                    ModelCapabilityProfile::new(ModelId::new("claude-test").unwrap())
                        .with_function_tools(CapabilityStatus::Supported)
                        .with_tool_choice_required(CapabilityStatus::Supported)
                        .with_tool_choice_specific(CapabilityStatus::Supported)
                        .with_parallel_tool_calls(CapabilityStatus::Supported)
                        .with_strict_tools(CapabilityStatus::Supported)
                        .with_vision_input(CapabilityStatus::Supported)
                        .with_response_format_json_object(CapabilityStatus::Supported)
                        .with_response_format_json_schema(CapabilityStatus::Supported),
                )
                .build()
                .unwrap();
        let request =
            GenerateRequest::new(ModelRef::new("test-only", "claude-test").unwrap(), messages)
                .with_options(options);
        CallPlanner::plan(&runtime, &request).unwrap()
    }

    fn plan_with_replay(messages: Vec<Message>) -> crate::execution::contract::ResolvedCallPlan {
        let mut compat = CompatPatch::from_source(PolicySource::ProviderProfile);
        compat.history_thinking_replay = Some(ThinkingReplayPolicy::SameSourceOnly);
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/v1/messages", "request-test-key")
                .unwrap()
                .with_compat(compat)
                .build()
                .unwrap();
        let request =
            GenerateRequest::new(ModelRef::new("test-only", "claude-test").unwrap(), messages)
                .with_options(GenerationOptions::new().with_max_output_tokens(128));
        CallPlanner::plan(&runtime, &request).unwrap()
    }

    fn anthropic_plan(options: GenerationOptions) -> crate::execution::contract::ResolvedCallPlan {
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/v1/messages", "request-test-key")
                .unwrap()
                .with_anthropic_messages()
                .with_model_capabilities(
                    ModelCapabilityProfile::new(ModelId::new("claude-test").unwrap())
                        .with_adaptive_thinking(CapabilityStatus::Supported)
                        .with_adaptive_thinking_effort(CapabilityStatus::Supported),
                )
                .build()
                .unwrap();
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "claude-test").unwrap(),
            vec![Message::user("Hello")],
        )
        .with_options(options);
        CallPlanner::plan(&runtime, &request).unwrap()
    }

    #[test]
    fn minimal_request_matches_golden_and_omits_unset_fields() {
        let plan = plan(
            vec![Message::user("Hello")],
            GenerationOptions::new().with_max_output_tokens(128),
        );
        let prepared = AnthropicMessagesDriver.prepare(&plan).unwrap();
        let actual: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/phase-5/anthropic-messages/request/minimal-text.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(prepared.request.protocol_headers.len(), 2);
        assert_eq!(
            prepared.facts.max_output_tokens_source,
            crate::protocol::MaxOutputTokensSource::Request
        );
        assert_eq!(
            prepared.request.operation,
            crate::protocol::ProtocolOperation::Messages
        );
    }

    #[test]
    fn typed_options_encode_adaptive_thinking_and_effort() {
        let options = AnthropicMessagesOptions::new()
            .with_adaptive_thinking(AnthropicThinkingDisplay::Summarized)
            .with_effort(AnthropicEffort::High);
        let plan = anthropic_plan(
            GenerationOptions::new()
                .with_max_output_tokens(128)
                .with_protocol_options(options),
        );
        let body: Value =
            serde_json::from_slice(&AnthropicMessagesDriver.prepare(&plan).unwrap().request.body)
                .unwrap();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["output_config"]["effort"], "high");
    }

    #[test]
    fn raw_extension_merges_last_and_reports_value_free_diagnostic() {
        let raw = AnthropicRawExtension::dangerous_from_object(
            serde_json::json!({"future_feature": {"enabled": true}}),
        )
        .unwrap();
        let options = AnthropicMessagesOptions::new().with_raw_extension(raw);
        assert_eq!(options.diagnostics().len(), 1);
        let plan = anthropic_plan(
            GenerationOptions::new()
                .with_max_output_tokens(128)
                .with_protocol_options(options),
        );
        let body: Value =
            serde_json::from_slice(&AnthropicMessagesDriver.prepare(&plan).unwrap().request.body)
                .unwrap();
        assert_eq!(body["future_feature"]["enabled"], true);
    }

    #[test]
    fn anthropic_options_fail_against_openai_runtime_before_preparation() {
        let runtime = TestOnlyProfile::localhost(
            "http://127.0.0.1:8787/chat/completions",
            "request-test-key",
        )
        .unwrap()
        .build()
        .unwrap();
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("Hello")],
        )
        .with_options(
            GenerationOptions::new().with_protocol_options(AnthropicMessagesOptions::new()),
        );
        let error = CallPlanner::plan(&runtime, &request).unwrap_err();
        assert!(matches!(
            error,
            crate::error::LlmError::Validation(ref validation)
                if validation.reason() == crate::error::ValidationReason::Conflict
        ));
    }

    #[test]
    fn system_is_top_level_and_never_a_message_role() {
        let plan = plan(
            vec![Message::system("policy"), Message::user("Hello")],
            GenerationOptions::new().with_max_output_tokens(128),
        );
        let prepared = AnthropicMessagesDriver.prepare(&plan).unwrap();
        let body: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(body["system"][0]["text"], "policy");
        assert!(
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| { matches!(message["role"].as_str(), Some("user" | "assistant")) })
        );
    }

    #[test]
    fn missing_max_tokens_fails_before_transport() {
        let plan = plan(vec![Message::user("Hello")], GenerationOptions::new());
        assert!(AnthropicMessagesDriver.prepare(&plan).is_err());
    }

    #[test]
    fn prepared_call_debug_redacts_body_and_header_values() {
        let plan = plan(
            vec![Message::user("request-body-canary")],
            GenerationOptions::new().with_max_output_tokens(128),
        );
        let prepared = AnthropicMessagesDriver.prepare(&plan).unwrap();
        let debug = format!("{prepared:?}");
        assert!(debug.contains("body_bytes"));
        assert!(!debug.contains("request-body-canary"));
        assert!(!debug.contains("request-test-key"));
    }

    fn schema() -> ToolSchema {
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"],
            "additionalProperties": false
        }))
        .unwrap()
    }

    #[test]
    fn system_tools_and_url_image_match_golden() {
        let messages = vec![
            Message::system("policy"),
            Message::new(
                MessageRole::User,
                vec![
                    ContentPart::text("Look"),
                    ContentPart::Image(
                        ImageContent::parse_url("https://example.com/test.png", ImageDetail::Auto)
                            .unwrap(),
                    ),
                ],
            ),
        ];
        let options = GenerationOptions::new()
            .with_max_output_tokens(128)
            .with_tools(vec![ToolDefinition::new(
                ToolName::new("lookup").unwrap(),
                schema(),
            )])
            .with_tool_choice(ToolChoice::Auto)
            .with_parallel_tool_calls(ParallelToolCalls::Disabled);
        let prepared = AnthropicMessagesDriver
            .prepare(&plan(messages, options))
            .unwrap();
        let actual: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/phase-5/anthropic-messages/request/system-tools-image.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn json_schema_matches_golden_and_json_object_is_rejected() {
        let output_schema = ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"],
            "additionalProperties": false
        }))
        .unwrap();
        let format = StructuredSchema::new("answer", None, output_schema, true).unwrap();
        let prepared = AnthropicMessagesDriver
            .prepare(&plan(
                vec![Message::user("Answer")],
                GenerationOptions::new()
                    .with_max_output_tokens(128)
                    .with_response_format(ResponseFormat::JsonSchema(format)),
            ))
            .unwrap();
        let actual: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/phase-5/anthropic-messages/request/structured-json-schema.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);

        assert!(
            AnthropicMessagesDriver
                .prepare(&plan(
                    vec![Message::user("Answer")],
                    GenerationOptions::new()
                        .with_max_output_tokens(128)
                        .with_response_format(ResponseFormat::JsonObject),
                ))
                .is_err()
        );
    }

    #[test]
    fn encoding_is_immutable_and_enforces_resolved_body_limit() {
        let messages = vec![Message::user("immutable")];
        let original = messages.clone();
        let mut plan = plan(
            messages,
            GenerationOptions::new().with_max_output_tokens(128),
        );
        AnthropicMessagesDriver.prepare(&plan).unwrap();
        assert_eq!(plan.planned.messages, original);

        plan.policy.limits.request.max_body_bytes = 1;
        assert!(AnthropicMessagesDriver.prepare(&plan).is_err());
    }

    #[test]
    fn non_auto_image_detail_is_rejected_before_transport() {
        let message = Message::new(
            MessageRole::User,
            vec![
                ContentPart::text("inspect"),
                ContentPart::Image(
                    ImageContent::parse_url("https://example.com/test.png", ImageDetail::High)
                        .unwrap(),
                ),
            ],
        );
        assert!(
            AnthropicMessagesDriver
                .prepare(&plan(
                    vec![message],
                    GenerationOptions::new().with_max_output_tokens(128),
                ))
                .is_err()
        );
    }

    #[test]
    fn history_folds_developer_and_merges_adjacent_roles_with_diagnostics() {
        let plan = plan(
            vec![
                Message::system("system"),
                Message::developer("developer"),
                Message::user("one"),
                Message::user("two"),
            ],
            GenerationOptions::new().with_max_output_tokens(128),
        );
        let history = super::super::history::plan_history(&plan).unwrap();
        assert_eq!(history.system.as_ref().unwrap().len(), 2);
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].content.len(), 2);
        assert!(history.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == crate::domain::DiagnosticCode::ConvertedDeveloperToSystem
        }));
        assert!(history.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == crate::domain::DiagnosticCode::MergedAdjacentMessages
        }));

        let openai: Value =
            serde_json::from_slice(&OpenAiChatDriver.prepare(&plan).unwrap().request.body).unwrap();
        let anthropic: Value =
            serde_json::from_slice(&AnthropicMessagesDriver.prepare(&plan).unwrap().request.body)
                .unwrap();
        assert!(
            openai["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| { message["role"] == "developer" })
        );
        assert!(
            anthropic["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| { matches!(message["role"].as_str(), Some("user" | "assistant")) })
        );
    }

    #[test]
    fn thinking_replay_is_same_source_only_and_opaque_value_is_redacted() {
        let source = SourceIdentity::new(
            ProviderId::new("test-only").unwrap(),
            ModelId::new("claude-test").unwrap(),
            ProtocolId::new("anthropic-messages").unwrap(),
        );
        let thinking = ThinkingContent::new("summary").with_opaque(OpaqueReasoning::new(
            Bytes::from_static(b"signature-canary"),
            source,
            false,
        ));
        let plan = plan_with_replay(vec![
            Message::user("question"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::Thinking(thinking)],
            ),
            Message::user("continue"),
        ]);
        let prepared = AnthropicMessagesDriver.prepare(&plan).unwrap();
        let body: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(
            body["messages"][1]["content"][0]["signature"],
            "signature-canary"
        );
        assert!(!format!("{plan:?}").contains("signature-canary"));

        let redacted = ThinkingContent::new("").with_opaque(OpaqueReasoning::new(
            Bytes::from_static(b"redacted-canary"),
            SourceIdentity::new(
                ProviderId::new("test-only").unwrap(),
                ModelId::new("claude-test").unwrap(),
                ProtocolId::new("anthropic-messages").unwrap(),
            ),
            true,
        ));
        let redacted_plan = plan_with_replay(vec![
            Message::user("question"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::Thinking(redacted)],
            ),
            Message::user("continue"),
        ]);
        let redacted_body: Value = serde_json::from_slice(
            &AnthropicMessagesDriver
                .prepare(&redacted_plan)
                .unwrap()
                .request
                .body,
        )
        .unwrap();
        assert_eq!(
            redacted_body["messages"][1]["content"][0]["type"],
            "redacted_thinking"
        );

        let other_source = SourceIdentity::new(
            ProviderId::new("other-provider").unwrap(),
            ModelId::new("claude-test").unwrap(),
            ProtocolId::new("anthropic-messages").unwrap(),
        );
        let cross_source = ThinkingContent::new("summary").with_opaque(OpaqueReasoning::new(
            Bytes::from_static(b"cross-source-canary"),
            other_source,
            false,
        ));
        let cross_plan = plan_with_replay(vec![
            Message::user("question"),
            Message::new(
                MessageRole::Assistant,
                vec![ContentPart::Thinking(cross_source)],
            ),
            Message::user("continue"),
        ]);
        let history = super::super::history::plan_history(&cross_plan).unwrap();
        assert_eq!(history.messages.len(), 1);
        assert!(history.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == crate::domain::DiagnosticCode::DroppedThinkingOpaque
        }));
    }

    #[test]
    fn tool_result_preserves_id_content_and_error_semantics() {
        let id = ToolCallId::new("toolu_test").unwrap();
        let name = ToolName::new("lookup").unwrap();
        let call = ToolCall::new(
            id.clone(),
            name.clone(),
            ToolArguments::from_raw_json(r#"{"query":"test"}"#).unwrap(),
        );
        let result = ToolResultMessage::error_text(id, name, "not found").unwrap();
        let plan = plan(
            vec![
                Message::user("look up"),
                Message::new(MessageRole::Assistant, vec![ContentPart::ToolCall(call)]),
                Message::from_tool_result(result),
            ],
            GenerationOptions::new().with_max_output_tokens(128),
        );
        let prepared = AnthropicMessagesDriver.prepare(&plan).unwrap();
        let body: Value = serde_json::from_slice(&prepared.request.body).unwrap();
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            body["messages"][2]["content"][0]["tool_use_id"],
            "toolu_test"
        );
        assert_eq!(body["messages"][2]["content"][0]["content"], "not found");
        assert_eq!(body["messages"][2]["content"][0]["is_error"], true);
    }
}
