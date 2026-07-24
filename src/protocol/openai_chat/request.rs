use std::borrow::Cow;

use bytes::Bytes;

use crate::domain::{
    ContentPart, GenerationOptions, ImageContent, ImageSource, Message, MessageRole,
    ReasoningEffort, StreamUsagePolicy, ThinkingRequest, ToolCall, ToolResultNamePolicy, content,
};
use crate::error::{LlmError, ProtocolError, ValidationError, ValidationReason};
use crate::execution::contract::ResolvedCallPlan;
use crate::provider::compat::ResolvedProviderRouting;
use crate::provider::{ModelBodyWireFormat, ProviderCapabilities, RequestCompat};

use super::compat::routing::ProviderRoutingWire;
use super::structured_wire::ResponseFormatWire;
use super::tool_wire::{encode_parallel_tool_calls, encode_tool_choice, encode_tools};
use super::wire::{
    AssistantToolCallWire, ChatCompletionRequestWire, ImageUrlWire, MessageContentPartWire,
    MessageContentWire, MessageWire, ReasoningEffortWire,
};

/// Encodes an already planned request without resolving policy or normalizing history.
pub(super) fn encode_planned_request(plan: &ResolvedCallPlan) -> Result<Bytes, LlmError> {
    let planned = &plan.planned;
    encode_request_parts(RequestEncodingContext {
        model: match plan.policy.compat.profile.request().model_body {
            ModelBodyWireFormat::Include => Some(plan.policy.target.wire_model.as_str()),
            ModelBodyWireFormat::Omit => None,
        },
        domain_messages: &planned.messages,
        options: &planned.options,
        capabilities: &plan.policy.capabilities,
        compat: *plan.policy.compat.profile.request(),
        tool_result_name: plan.policy.compat.profile.history().tool_result_name,
        default_max_output_tokens: plan.policy.limits.model.default_max_output_tokens,
        max_body_bytes: plan.policy.limits.request.max_body_bytes,
        provider_routing: plan.policy.provider_routing.as_ref(),
    })
}

#[derive(Clone, Copy)]
struct RequestEncodingContext<'a> {
    model: Option<&'a str>,
    domain_messages: &'a [Message],
    options: &'a GenerationOptions,
    capabilities: &'a ProviderCapabilities,
    compat: RequestCompat,
    tool_result_name: ToolResultNamePolicy,
    default_max_output_tokens: Option<u32>,
    max_body_bytes: usize,
    provider_routing: Option<&'a ResolvedProviderRouting>,
}

fn encode_request_parts(context: RequestEncodingContext<'_>) -> Result<Bytes, LlmError> {
    let RequestEncodingContext {
        model,
        domain_messages,
        options,
        capabilities,
        compat,
        tool_result_name,
        default_max_output_tokens,
        max_body_bytes,
        provider_routing,
    } = context;
    let mut messages = Vec::with_capacity(domain_messages.len());
    for (index, message) in domain_messages.iter().enumerate() {
        messages.push(encode_message(message, index, tool_result_name)?);
    }
    let capabilities_for_tools = capabilities.generation_options();
    let tools = encode_tools(options.tools(), &capabilities_for_tools)?;
    let tool_choice = encode_tool_choice(
        options.tools(),
        options.tool_choice(),
        &capabilities_for_tools,
    )?;
    let parallel_tool_calls =
        encode_parallel_tool_calls(options.parallel_tool_calls(), &capabilities_for_tools)?;
    let wire = ChatCompletionRequestWire::new(
        model,
        messages,
        options.temperature(),
        options.max_output_tokens().or(default_max_output_tokens),
        tools,
        tool_choice,
        parallel_tool_calls,
        ResponseFormatWire::from_domain(options.response_format()),
        encode_reasoning_effort(options.reasoning()),
        compat.max_output_tokens,
        matches!(compat.stream_usage, StreamUsagePolicy::IncludeUsage),
        provider_routing.map(ProviderRoutingWire::from),
    );
    let body = serde_json::to_vec(&wire).map_err(|_| {
        LlmError::from(ProtocolError::new(
            "failed to serialize planned OpenAI Chat request",
        ))
    })?;
    if body.len() > max_body_bytes {
        return Err(ValidationError::new(
            "request_body",
            ValidationReason::OutOfRange,
            "encoded request body exceeds the resolved size limit",
        )
        .into());
    }
    Ok(Bytes::from(body))
}

fn encode_message(
    message: &Message,
    index: usize,
    tool_result_name: ToolResultNamePolicy,
) -> Result<MessageWire<'_>, LlmError> {
    match message.role() {
        MessageRole::Developer | MessageRole::System => {
            let text = single_text(message, index)?;
            Ok(MessageWire::text(message.role(), text))
        }
        MessageRole::User => encode_user_message(message),
        MessageRole::Assistant => encode_assistant_message(message, index),
        MessageRole::Tool => {
            let result = message.tool_result().ok_or_else(|| {
                ProtocolError::new("tool role message is missing a tool result payload")
            })?;
            let text = match result.content() {
                [ContentPart::Text { text }] if !text.is_empty() => text.as_str(),
                _ => {
                    return Err(ProtocolError::new(
                        "official tool results require exactly one non-empty text part",
                    )
                    .into());
                }
            };
            Ok(MessageWire::tool_result(
                result.tool_call_id().as_str(),
                text,
                super::compat::request::tool_result_name(
                    result.tool_name().as_str(),
                    tool_result_name,
                ),
            ))
        }
    }
}

fn encode_user_message(message: &Message) -> Result<MessageWire<'_>, LlmError> {
    let parts = message.content();
    if parts.is_empty() {
        return Err(ProtocolError::new("user message content is empty").into());
    }
    if let [ContentPart::Text { text }] = parts {
        if text.trim().is_empty() {
            return Err(ProtocolError::new("user text must contain non-whitespace").into());
        }
        return Ok(MessageWire::text(MessageRole::User, text));
    }

    let mut wire_parts = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentPart::Text { text } => {
                wire_parts.push(MessageContentPartWire::Text { text });
            }
            ContentPart::Image(image) => {
                wire_parts.push(encode_image_part(image)?);
            }
            ContentPart::Thinking(_) | ContentPart::Refusal(_) | ContentPart::ToolCall(_) => {
                return Err(
                    ProtocolError::new("user messages only accept text and image parts").into(),
                );
            }
        }
    }
    Ok(MessageWire::parts(MessageRole::User, wire_parts))
}

fn encode_assistant_message(message: &Message, index: usize) -> Result<MessageWire<'_>, LlmError> {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut seen_tool_call = false;

    for (part_index, part) in message.content().iter().enumerate() {
        match part {
            ContentPart::Text { text } => {
                if seen_tool_call {
                    return Err(ProtocolError::new(
                        "assistant text after tool calls is not allowed",
                    )
                    .into());
                }
                text_parts.push(text.as_str());
            }
            ContentPart::ToolCall(call) => {
                seen_tool_call = true;
                tool_calls.push(encode_assistant_tool_call(call));
            }
            ContentPart::Image(_) | ContentPart::Thinking(_) | ContentPart::Refusal(_) => {
                return Err(ValidationError::new(
                    format!("messages[{index}].content[{part_index}]"),
                    ValidationReason::TextPartCount,
                    "official assistant history only accepts text and tool calls",
                )
                .into());
            }
        }
    }

    if tool_calls.is_empty() {
        if text_parts.is_empty() {
            return Err(ProtocolError::new("assistant message content is empty").into());
        }
        if text_parts.len() == 1 {
            return Ok(MessageWire::text(MessageRole::Assistant, text_parts[0]));
        }
        return Ok(MessageWire::assistant(
            Some(MessageContentWire::OwnedText(text_parts.concat())),
            None,
        ));
    }

    let content = if text_parts.is_empty() {
        None
    } else if text_parts.len() == 1 {
        Some(MessageContentWire::Text(text_parts[0]))
    } else {
        Some(MessageContentWire::OwnedText(text_parts.concat()))
    };
    Ok(MessageWire::assistant(content, Some(tool_calls)))
}

fn encode_assistant_tool_call(call: &ToolCall) -> AssistantToolCallWire<'_> {
    AssistantToolCallWire::new(
        call.id().as_str(),
        call.name().as_str(),
        call.arguments().raw_json(),
    )
}

fn encode_image_part(image: &ImageContent) -> Result<MessageContentPartWire<'_>, LlmError> {
    let url = match image.source() {
        ImageSource::Url(url) => Cow::Borrowed(url.as_str()),
        ImageSource::DataUrl(data_url) => {
            content::decode_validated_data_url(data_url).map_err(LlmError::from)?;
            Cow::Borrowed(data_url.as_str())
        }
        ImageSource::Inline { mime, bytes } => {
            Cow::Owned(content::encode_inline_data_url(*mime, bytes))
        }
    };
    Ok(MessageContentPartWire::ImageUrl {
        image_url: ImageUrlWire::new(url, image.detail()),
    })
}

fn encode_reasoning_effort(request: ThinkingRequest) -> Option<ReasoningEffortWire> {
    match request {
        ThinkingRequest::ProviderDefault => None,
        ThinkingRequest::Disabled => Some(ReasoningEffortWire::None),
        ThinkingRequest::Effort(effort) => Some(match effort {
            ReasoningEffort::None => ReasoningEffortWire::None,
            ReasoningEffort::Minimal => ReasoningEffortWire::Minimal,
            ReasoningEffort::Low => ReasoningEffortWire::Low,
            ReasoningEffort::Medium => ReasoningEffortWire::Medium,
            ReasoningEffort::High => ReasoningEffortWire::High,
            ReasoningEffort::XHigh => ReasoningEffortWire::XHigh,
            ReasoningEffort::Max => ReasoningEffortWire::Max,
        }),
    }
}

fn single_text(message: &Message, index: usize) -> Result<&str, LlmError> {
    match message.content() {
        [ContentPart::Text { text }] if !text.is_empty() => Ok(text.as_str()),
        _ => Err(ValidationError::new(
            format!("messages[{index}].content"),
            ValidationReason::TextPartCount,
            "developer and system messages require exactly one non-empty text part",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use bytes::Bytes;
    use serde_json::Value;

    use crate::domain::{
        CapabilityStatus, ContentPart, GenerateRequest, GenerationOptions, ImageContent,
        ImageDetail, ImageMime, Message, MessageRole, ModelId, ModelRef, ParallelToolCalls,
        ReasoningEffort, ReasoningEffortSupport, RequestMetadata, ResponseFormat, StructuredSchema,
        ThinkingRequest, ToolChoice, ToolDefinition, ToolName, ToolSchema,
    };
    use crate::execution::planner::CallPlanner;
    use crate::protocol::ProtocolOperation;
    use crate::protocol::openai_chat::OpenAiChatDriver;
    use crate::provider::{ModelCapabilityProfile, TestOnlyProfile};

    const MINIMAL: &str =
        include_str!("../../../tests/fixtures/requests/openai_chat/minimal-user.json");
    const ALL_ROLES: &str =
        include_str!("../../../tests/fixtures/requests/openai_chat/all-roles.json");
    const TEMPERATURE_ONLY: &str =
        include_str!("../../../tests/fixtures/requests/openai_chat/temperature-only.json");
    const MAX_TOKENS_ONLY: &str =
        include_str!("../../../tests/fixtures/requests/openai_chat/max-tokens-only.json");
    const ALL_OPTIONS: &str =
        include_str!("../../../tests/fixtures/requests/openai_chat/all-options.json");
    const TOOL_MINIMAL_AUTO: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-minimal-auto.json");
    const TOOL_NONE: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-none.json");
    const TOOL_REQUIRED: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-required.json");
    const TOOL_SPECIFIC: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-specific.json");
    const TOOL_STRICT: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-strict.json");
    const PARALLEL_TOOLS: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/parallel-tools-enabled.json");
    const TOOL_DESCRIPTION_OMITTED: &str = include_str!(
        "../../../tests/fixtures/phase-2/requests/tools/tool-description-omitted.json"
    );
    const TOOL_SCHEMA_NESTED: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/tools/tool-schema-nested.json");
    const IMAGE_URL: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/multimodal/text-one-url-image.json");
    const IMAGE_INLINE: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/multimodal/text-inline-image.json");
    const IMAGE_INTERLEAVED: &str = include_str!(
        "../../../tests/fixtures/phase-2/requests/multimodal/text-image-interleaved.json"
    );
    const REASONING_HIGH: &str = include_str!(
        "../../../tests/fixtures/phase-2/requests/thinking/reasoning-effort-high.json"
    );
    const JSON_OBJECT: &str =
        include_str!("../../../tests/fixtures/phase-2/requests/structured-output/json-object.json");
    const JSON_SCHEMA_STRICT: &str = include_str!(
        "../../../tests/fixtures/phase-2/requests/structured-output/json-schema-strict.json"
    );

    fn capabilities() -> ModelCapabilityProfile {
        ModelCapabilityProfile::new(ModelId::new("gpt-test").unwrap())
    }

    fn tools_capabilities() -> ModelCapabilityProfile {
        capabilities()
            .with_function_tools(CapabilityStatus::Supported)
            .with_tool_choice_required(CapabilityStatus::Supported)
            .with_tool_choice_specific(CapabilityStatus::Supported)
            .with_parallel_tool_calls(CapabilityStatus::Supported)
            .with_strict_tools(CapabilityStatus::Supported)
    }

    fn vision_capabilities() -> ModelCapabilityProfile {
        capabilities()
            .with_vision_input(CapabilityStatus::Supported)
            .with_image_detail_original(CapabilityStatus::Supported)
    }

    fn reasoning_capabilities() -> ModelCapabilityProfile {
        capabilities().with_reasoning_efforts(ReasoningEffortSupport::Supported(BTreeSet::from([
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ])))
    }

    fn structured_capabilities() -> ModelCapabilityProfile {
        capabilities()
            .with_response_format_json_object(CapabilityStatus::Supported)
            .with_response_format_json_schema(CapabilityStatus::Supported)
    }

    fn weather_schema() -> ToolSchema {
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string" }
            },
            "required": ["city"],
            "additionalProperties": false
        }))
        .unwrap()
    }

    fn weather_tool() -> ToolDefinition {
        ToolDefinition::new(ToolName::new("get_weather").unwrap(), weather_schema())
    }

    fn request(messages: Vec<Message>) -> GenerateRequest {
        GenerateRequest::new(ModelRef::new("test-only", "gpt-test").unwrap(), messages)
    }

    fn prepare(
        request: &GenerateRequest,
        capabilities: ModelCapabilityProfile,
    ) -> crate::protocol::PreparedCall {
        prepare_with_key(request, capabilities, "test-key")
    }

    fn prepare_with_key(
        request: &GenerateRequest,
        capabilities: ModelCapabilityProfile,
        key: &str,
    ) -> crate::protocol::PreparedCall {
        let runtime = TestOnlyProfile::localhost("http://127.0.0.1:8787/v1/chat/completions", key)
            .unwrap()
            .with_model_capabilities(capabilities)
            .build()
            .unwrap();
        let plan = CallPlanner::plan(&runtime, request).unwrap();
        OpenAiChatDriver.prepare(&plan).unwrap()
    }

    fn encoded_value(request: &GenerateRequest, capabilities: ModelCapabilityProfile) -> Value {
        let prepared = prepare(request, capabilities);
        serde_json::from_slice(&prepared.request.body).unwrap()
    }

    fn golden(value: &Value, fixture: &str) {
        let expected: Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(value, &expected);
    }

    fn png_bytes() -> Bytes {
        Bytes::from_static(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 1, 2, 3])
    }

    #[test]
    fn minimal_request_matches_golden_and_protocol_intent() {
        let prepared = prepare(&request(vec![Message::user("Hello")]), capabilities());
        let value: Value = serde_json::from_slice(&prepared.request.body).unwrap();

        golden(&value, MINIMAL);
        assert_eq!(prepared.request.method, http::Method::POST);
        assert_eq!(
            prepared.request.operation,
            ProtocolOperation::ChatCompletions
        );
        assert_eq!(prepared.request.protocol_headers.len(), 2);
    }

    #[test]
    fn all_roles_preserve_order_and_exact_text() {
        let request = request(vec![
            Message::developer("developer\ntext"),
            Message::system("system text"),
            Message::user("  user text  "),
            Message::assistant("assistant text"),
        ]);
        golden(&encoded_value(&request, capabilities()), ALL_ROLES);
    }

    #[test]
    fn option_combinations_match_goldens_and_none_is_omitted() {
        let base = request(vec![Message::user("Hello")]);

        let temperature = base
            .clone()
            .with_options(GenerationOptions::new().with_temperature(0.75));
        golden(
            &encoded_value(&temperature, capabilities()),
            TEMPERATURE_ONLY,
        );

        let max_tokens = base
            .clone()
            .with_options(GenerationOptions::new().with_max_output_tokens(256));
        golden(&encoded_value(&max_tokens, capabilities()), MAX_TOKENS_ONLY);

        let all = base.clone().with_options(
            GenerationOptions::new()
                .with_temperature(0.75)
                .with_max_output_tokens(256),
        );
        golden(&encoded_value(&all, capabilities()), ALL_OPTIONS);

        let minimal = encoded_value(&base, capabilities());
        golden(&minimal, MINIMAL);
        assert_no_null(&minimal);
        assert!(minimal.get("temperature").is_none());
        assert!(minimal.get("max_completion_tokens").is_none());
        assert!(minimal.get("reasoning_effort").is_none());
    }

    #[test]
    fn forbidden_fields_and_provider_metadata_never_enter_json() {
        let value = encoded_value(&request(vec![Message::user("Hello")]), capabilities());
        let forbidden = [
            "max_tokens",
            "tools",
            "tool_choice",
            "reasoning",
            "thinking",
            "provider",
            "extra_body",
            "prompt_cache_key",
            "store",
        ];
        assert_no_forbidden_keys(&value, &forbidden);
        assert_eq!(value["model"], "gpt-test");
        assert!(!value.to_string().contains("test-only"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn tool_request_goldens_match_and_omit_defaults() {
        let capabilities = tools_capabilities();
        let base = request(vec![Message::user("Hello")]);

        let auto = base
            .clone()
            .with_options(GenerationOptions::new().with_tools(vec![weather_tool()]));
        golden(
            &encoded_value(&auto, capabilities.clone()),
            TOOL_MINIMAL_AUTO,
        );

        let none = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::None),
        );
        golden(&encoded_value(&none, capabilities.clone()), TOOL_NONE);

        let required = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::Required),
        );
        golden(
            &encoded_value(&required, capabilities.clone()),
            TOOL_REQUIRED,
        );

        let specific = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::Specific {
                    name: ToolName::new("get_weather").unwrap(),
                }),
        );
        golden(
            &encoded_value(&specific, capabilities.clone()),
            TOOL_SPECIFIC,
        );

        let strict = base
            .clone()
            .with_options(GenerationOptions::new().with_tools(vec![
            weather_tool()
                .with_description("Get weather")
                .unwrap()
                .with_strict(true),
        ]));
        golden(&encoded_value(&strict, capabilities.clone()), TOOL_STRICT);

        let parallel = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![
                    weather_tool(),
                    ToolDefinition::new(
                        ToolName::new("get_time").unwrap(),
                        ToolSchema::new(serde_json::json!({
                            "type": "object",
                            "properties": {
                                "timezone": { "type": "string" }
                            },
                            "required": ["timezone"],
                            "additionalProperties": false
                        }))
                        .unwrap(),
                    ),
                ])
                .with_parallel_tool_calls(ParallelToolCalls::Enabled),
        );
        golden(
            &encoded_value(&parallel, capabilities.clone()),
            PARALLEL_TOOLS,
        );

        let omitted = base
            .clone()
            .with_options(GenerationOptions::new().with_tools(vec![
            ToolDefinition::new(
                ToolName::new("lookup").unwrap(),
                ToolSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": false
                }))
                .unwrap(),
            ),
        ]));
        golden(
            &encoded_value(&omitted, capabilities.clone()),
            TOOL_DESCRIPTION_OMITTED,
        );

        let nested = base.with_options(GenerationOptions::new().with_tools(vec![
            ToolDefinition::new(
                ToolName::new("search").unwrap(),
                ToolSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "object",
                            "properties": {
                                "term": { "type": "string" },
                                "limit": { "type": "integer" }
                            },
                            "required": ["term", "limit"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }))
                .unwrap(),
            ),
        ]));
        golden(&encoded_value(&nested, capabilities), TOOL_SCHEMA_NESTED);
    }

    #[test]
    fn multimodal_request_goldens_preserve_content_order() {
        let interleaved = request(vec![Message::new(
            MessageRole::User,
            vec![
                ContentPart::text("compare"),
                ContentPart::Image(
                    ImageContent::parse_url("https://example.com/a.png", ImageDetail::High)
                        .unwrap(),
                ),
                ContentPart::text("with this"),
            ],
        )]);
        golden(
            &encoded_value(&interleaved, vision_capabilities()),
            IMAGE_INTERLEAVED,
        );

        let url_only = request(vec![Message::new(
            MessageRole::User,
            vec![
                ContentPart::text("describe"),
                ContentPart::Image(
                    ImageContent::parse_url("https://example.com/cat.png", ImageDetail::Auto)
                        .unwrap(),
                ),
            ],
        )]);
        golden(&encoded_value(&url_only, vision_capabilities()), IMAGE_URL);

        let inline = request(vec![Message::new(
            MessageRole::User,
            vec![
                ContentPart::text("inline"),
                ContentPart::Image(
                    ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Low)
                        .unwrap(),
                ),
            ],
        )]);
        golden(&encoded_value(&inline, vision_capabilities()), IMAGE_INLINE);
        let inline_value = encoded_value(&inline, vision_capabilities());
        let url = inline_value["messages"][0]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let payload = url.strip_prefix("data:image/png;base64,").unwrap();
        assert_eq!(
            BASE64_STANDARD.decode(payload).unwrap(),
            png_bytes().as_ref()
        );
    }

    #[test]
    fn reasoning_effort_is_omitted_by_default_and_encoded_when_supported() {
        let base = request(vec![Message::user("Hello")]);
        let default_value = encoded_value(&base, reasoning_capabilities());
        assert!(default_value.get("reasoning_effort").is_none());

        let high = base.with_options(
            GenerationOptions::new().with_reasoning(ThinkingRequest::Effort(ReasoningEffort::High)),
        );
        golden(
            &encoded_value(&high, reasoning_capabilities()),
            REASONING_HIGH,
        );

        let disabled = request(vec![Message::user("Hello")])
            .with_options(GenerationOptions::new().with_reasoning(ThinkingRequest::Disabled));
        let disabled_value = encoded_value(&disabled, reasoning_capabilities());
        assert_eq!(disabled_value["reasoning_effort"], "none");
    }

    #[test]
    fn structured_output_request_goldens_match_and_omit_text_default() {
        let capabilities = structured_capabilities();
        let base = request(vec![Message::user("Return JSON")]);
        assert!(
            encoded_value(&base, capabilities.clone())
                .get("response_format")
                .is_none()
        );

        let object = base.clone().with_options(
            GenerationOptions::new().with_response_format(ResponseFormat::JsonObject),
        );
        golden(&encoded_value(&object, capabilities.clone()), JSON_OBJECT);

        let schema = StructuredSchema::new(
            "answer_object",
            None,
            ToolSchema::new(serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "integer" }
                },
                "required": ["answer"],
                "additionalProperties": false
            }))
            .unwrap(),
            true,
        )
        .unwrap();
        let schema_request = request(vec![Message::user("Return an answer object")]).with_options(
            GenerationOptions::new().with_response_format(ResponseFormat::JsonSchema(schema)),
        );
        golden(
            &encoded_value(&schema_request, capabilities),
            JSON_SCHEMA_STRICT,
        );
    }

    #[test]
    fn p1_text_request_remains_unchanged_when_tools_are_absent() {
        golden(
            &encoded_value(&request(vec![Message::user("Hello")]), capabilities()),
            MINIMAL,
        );
    }

    #[test]
    fn encoding_does_not_modify_the_domain_request() {
        let mut metadata = RequestMetadata::new();
        metadata.insert("trace", "local-only").unwrap();
        let request = request(vec![Message::user("Hello")]).with_options(
            GenerationOptions::new()
                .with_temperature(0.5)
                .with_max_output_tokens(32)
                .with_metadata(metadata),
        );
        let before_model = request.model().clone();
        let before_messages = request.messages().to_vec();
        let before_temperature = request.options().temperature();
        let before_max = request.options().max_output_tokens();
        let before_metadata: Vec<_> = request.options().metadata().iter().collect();

        prepare(&request, capabilities());

        assert_eq!(request.model(), &before_model);
        assert_eq!(request.messages(), before_messages);
        assert_eq!(request.options().temperature(), before_temperature);
        assert_eq!(request.options().max_output_tokens(), before_max);
        assert_eq!(
            request.options().metadata().iter().collect::<Vec<_>>(),
            before_metadata
        );
    }

    #[test]
    fn api_key_cannot_enter_the_encoded_body() {
        let secret = "********************************";
        let prepared = prepare_with_key(
            &request(vec![Message::user("Hello")]),
            capabilities(),
            secret,
        );
        assert!(!String::from_utf8_lossy(&prepared.request.body).contains(secret));
    }

    fn assert_no_null(value: &Value) {
        match value {
            Value::Null => panic!("wire request contains null"),
            Value::Array(values) => values.iter().for_each(assert_no_null),
            Value::Object(values) => values.values().for_each(assert_no_null),
            _ => {}
        }
    }

    fn assert_no_forbidden_keys(value: &Value, forbidden: &[&str]) {
        match value {
            Value::Array(values) => values
                .iter()
                .for_each(|value| assert_no_forbidden_keys(value, forbidden)),
            Value::Object(values) => {
                for (key, value) in values {
                    assert!(!forbidden.contains(&key.as_str()), "forbidden key: {key}");
                    assert_no_forbidden_keys(value, forbidden);
                }
            }
            _ => {}
        }
    }
}
