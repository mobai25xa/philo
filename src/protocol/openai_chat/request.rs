use std::borrow::Cow;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, header};

use crate::domain::{
    CapabilityStatus, ContentPart, GenerateRequest, ImageContent, ImageSource, Message,
    MessageRole, ReasoningEffort, ResourceLimits, ThinkingRequest, ToolCall, content,
};
use crate::error::{CapabilityError, LlmError, ProtocolError, ValidationError, ValidationReason};
use crate::provider::ProviderCapabilities;

use super::structured_wire::ResponseFormatWire;
use super::tool_wire::{encode_parallel_tool_calls, encode_tool_choice, encode_tools};
use super::wire::{
    AssistantToolCallWire, ChatCompletionRequestWire, ImageUrlWire, MessageContentPartWire,
    MessageContentWire, MessageWire, ReasoningEffortWire,
};

const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

/// The protocol intent produced before endpoint, authentication, and transport assembly.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct EncodedOpenAiChatRequest {
    pub(crate) method: Method,
    pub(crate) relative_path: &'static str,
    pub(crate) protocol_headers: HeaderMap,
    pub(crate) body: Bytes,
}

/// Concrete phase-one `OpenAI` Chat request encoder.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpenAiChatRequestAdapter;

impl OpenAiChatRequestAdapter {
    /// Validates capabilities and encodes a domain request without performing I/O.
    pub(crate) fn encode(
        request: &GenerateRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<EncodedOpenAiChatRequest, LlmError> {
        request.validate(&capabilities.generation_options())?;
        require_capability("stream", "streaming", capabilities.streaming)?;
        require_capability(
            "stream_options.include_usage",
            "streaming_usage",
            capabilities.streaming_usage,
        )?;

        let mut messages = Vec::with_capacity(request.messages().len());
        for (index, message) in request.messages().iter().enumerate() {
            if message.role() == MessageRole::Developer {
                require_capability(
                    &format!("messages[{index}].role"),
                    "developer_role",
                    capabilities.developer_role,
                )?;
            }
            messages.push(encode_message(message, index)?);
        }

        let capabilities_for_tools = capabilities.generation_options();
        let tools = encode_tools(request.options().tools(), &capabilities_for_tools)?;
        let tool_choice = encode_tool_choice(
            request.options().tools(),
            request.options().tool_choice(),
            &capabilities_for_tools,
        )?;
        let parallel_tool_calls = encode_parallel_tool_calls(
            request.options().parallel_tool_calls(),
            &capabilities_for_tools,
        )?;
        let reasoning_effort = encode_reasoning_effort(request.options().reasoning());
        let response_format = ResponseFormatWire::from_domain(request.options().response_format());

        let wire = ChatCompletionRequestWire::new(
            request.model().model().as_str(),
            messages,
            request.options().temperature(),
            request.options().max_output_tokens(),
            tools,
            tool_choice,
            parallel_tool_calls,
            response_format,
            reasoning_effort,
        );
        let body = serde_json::to_vec(&wire).map_err(|_| {
            LlmError::from(ProtocolError::new(
                "failed to serialize validated OpenAI Chat request",
            ))
        })?;
        if body.len() > ResourceLimits::official().max_request_body_bytes {
            return Err(ValidationError::new(
                "request_body",
                ValidationReason::OutOfRange,
                "encoded request body exceeds the frozen size limit",
            )
            .into());
        }

        let mut protocol_headers = HeaderMap::with_capacity(2);
        protocol_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        protocol_headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );

        Ok(EncodedOpenAiChatRequest {
            method: Method::POST,
            relative_path: CHAT_COMPLETIONS_PATH,
            protocol_headers,
            body: Bytes::from(body),
        })
    }
}

fn encode_message(message: &Message, index: usize) -> Result<MessageWire<'_>, LlmError> {
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

fn require_capability(
    field: &str,
    capability: &str,
    status: CapabilityStatus,
) -> Result<(), LlmError> {
    match status {
        CapabilityStatus::Supported => Ok(()),
        CapabilityStatus::Unsupported => {
            Err(CapabilityError::new(field, capability, "Unsupported").into())
        }
        CapabilityStatus::Unknown => Err(CapabilityError::new(field, capability, "Unknown").into()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use bytes::Bytes;
    use http::header;
    use serde_json::Value;

    use crate::domain::{
        CapabilityStatus, ContentPart, GenerateRequest, GenerationOptions, ImageContent,
        ImageDetail, ImageMime, Message, MessageRole, ModelRef, ParallelToolCalls, ReasoningEffort,
        ReasoningEffortSupport, RequestMetadata, ResponseFormat, StructuredSchema, ThinkingRequest,
        ToolChoice, ToolDefinition, ToolName, ToolSchema,
    };
    use crate::error::LlmError;
    use crate::provider::{OfficialOpenAiProfile, ProviderCapabilities};

    use super::OpenAiChatRequestAdapter;

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

    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            developer_role: CapabilityStatus::Supported,
            temperature: CapabilityStatus::Supported,
            max_completion_tokens: CapabilityStatus::Supported,
            streaming: CapabilityStatus::Supported,
            streaming_usage: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
        }
    }

    fn tools_capabilities() -> ProviderCapabilities {
        let mut current = capabilities();
        current.function_tools = CapabilityStatus::Supported;
        current.tool_choice_required = CapabilityStatus::Supported;
        current.tool_choice_specific = CapabilityStatus::Supported;
        current.parallel_tool_calls = CapabilityStatus::Supported;
        current.strict_tools = CapabilityStatus::Supported;
        current
    }

    fn vision_capabilities() -> ProviderCapabilities {
        let mut current = capabilities();
        current.vision_input = CapabilityStatus::Supported;
        current.image_detail_original = CapabilityStatus::Supported;
        current
    }

    fn reasoning_capabilities() -> ProviderCapabilities {
        let mut current = capabilities();
        current.reasoning_efforts = ReasoningEffortSupport::Supported(BTreeSet::from([
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ]));
        current
    }

    fn structured_capabilities() -> ProviderCapabilities {
        let mut current = capabilities();
        current.response_format_json_object = CapabilityStatus::Supported;
        current.response_format_json_schema = CapabilityStatus::Supported;
        current
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
        GenerateRequest::new(
            ModelRef::new("routing-alias", "gpt-test").unwrap(),
            messages,
        )
    }

    fn encoded_value(request: &GenerateRequest, capabilities: &ProviderCapabilities) -> Value {
        let encoded = OpenAiChatRequestAdapter::encode(request, capabilities).unwrap();
        serde_json::from_slice(&encoded.body).unwrap()
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
        let encoded = OpenAiChatRequestAdapter::encode(
            &request(vec![Message::user("Hello")]),
            &capabilities(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&encoded.body).unwrap();

        golden(&value, MINIMAL);
        assert_eq!(encoded.method, http::Method::POST);
        assert_eq!(encoded.relative_path, "/chat/completions");
        assert_eq!(encoded.protocol_headers.len(), 2);
        assert_eq!(
            encoded.protocol_headers.get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            encoded.protocol_headers.get(header::ACCEPT).unwrap(),
            "text/event-stream"
        );
    }

    #[test]
    fn all_roles_preserve_order_and_exact_text() {
        let request = request(vec![
            Message::developer("developer\ntext"),
            Message::system("system text"),
            Message::user("  user text  "),
            Message::assistant("assistant text"),
        ]);
        golden(&encoded_value(&request, &capabilities()), ALL_ROLES);
    }

    #[test]
    fn option_combinations_match_goldens_and_none_is_omitted() {
        let base = request(vec![Message::user("Hello")]);

        let temperature = base
            .clone()
            .with_options(GenerationOptions::new().with_temperature(0.75));
        golden(
            &encoded_value(&temperature, &capabilities()),
            TEMPERATURE_ONLY,
        );

        let max_tokens = base
            .clone()
            .with_options(GenerationOptions::new().with_max_output_tokens(256));
        golden(
            &encoded_value(&max_tokens, &capabilities()),
            MAX_TOKENS_ONLY,
        );

        let all = base.clone().with_options(
            GenerationOptions::new()
                .with_temperature(0.75)
                .with_max_output_tokens(256),
        );
        golden(&encoded_value(&all, &capabilities()), ALL_OPTIONS);

        let minimal = encoded_value(&base, &capabilities());
        golden(&minimal, MINIMAL);
        assert_no_null(&minimal);
        assert!(minimal.get("temperature").is_none());
        assert!(minimal.get("max_completion_tokens").is_none());
        assert!(minimal.get("reasoning_effort").is_none());
    }

    #[test]
    fn optional_capabilities_are_ignored_only_when_the_option_is_absent() {
        let mut unavailable = capabilities();
        unavailable.temperature = CapabilityStatus::Unknown;
        unavailable.max_completion_tokens = CapabilityStatus::Unsupported;
        assert!(
            OpenAiChatRequestAdapter::encode(&request(vec![Message::user("Hello")]), &unavailable,)
                .is_ok()
        );
    }

    #[test]
    fn unsupported_and_unknown_options_fail_before_transport() {
        for status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
            let transport_calls = Cell::new(0_u32);
            let mut temperature_capabilities = capabilities();
            temperature_capabilities.temperature = status;
            let temperature = request(vec![Message::user("Hello")])
                .with_options(GenerationOptions::new().with_temperature(1.0));
            let result = OpenAiChatRequestAdapter::encode(&temperature, &temperature_capabilities)
                .map(|_| transport_calls.set(transport_calls.get() + 1));
            assert_capability_error(&result, "temperature", status);
            assert_eq!(transport_calls.get(), 0);

            let mut max_capabilities = capabilities();
            max_capabilities.max_completion_tokens = status;
            let max_tokens = request(vec![Message::user("Hello")])
                .with_options(GenerationOptions::new().with_max_output_tokens(8));
            assert_capability_error(
                &OpenAiChatRequestAdapter::encode(&max_tokens, &max_capabilities),
                "max_output_tokens",
                status,
            );
        }
    }

    #[test]
    fn developer_role_never_silently_downgrades() {
        for status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
            let mut current = capabilities();
            current.developer_role = status;
            let result = OpenAiChatRequestAdapter::encode(
                &request(vec![
                    Message::developer("instruction"),
                    Message::user("Hello"),
                ]),
                &current,
            );
            assert_capability_error(&result, "messages[0].role", status);
        }
    }

    #[test]
    fn fixed_streaming_capabilities_fail_closed() {
        for status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
            let mut streaming = capabilities();
            streaming.streaming = status;
            assert_capability_error(
                &OpenAiChatRequestAdapter::encode(
                    &request(vec![Message::user("Hello")]),
                    &streaming,
                ),
                "stream",
                status,
            );

            let mut usage = capabilities();
            usage.streaming_usage = status;
            assert_capability_error(
                &OpenAiChatRequestAdapter::encode(&request(vec![Message::user("Hello")]), &usage),
                "stream_options.include_usage",
                status,
            );
        }
    }

    #[test]
    fn forbidden_fields_and_provider_metadata_never_enter_json() {
        let value = encoded_value(&request(vec![Message::user("Hello")]), &capabilities());
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
        assert!(!value.to_string().contains("routing-alias"));
    }

    #[test]
    fn tool_request_goldens_match_and_omit_defaults() {
        let capabilities = tools_capabilities();
        let base = request(vec![Message::user("Hello")]);

        let auto = base
            .clone()
            .with_options(GenerationOptions::new().with_tools(vec![weather_tool()]));
        golden(&encoded_value(&auto, &capabilities), TOOL_MINIMAL_AUTO);

        let none = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::None),
        );
        golden(&encoded_value(&none, &capabilities), TOOL_NONE);

        let required = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::Required),
        );
        golden(&encoded_value(&required, &capabilities), TOOL_REQUIRED);

        let specific = base.clone().with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::Specific {
                    name: ToolName::new("get_weather").unwrap(),
                }),
        );
        golden(&encoded_value(&specific, &capabilities), TOOL_SPECIFIC);

        let strict = base
            .clone()
            .with_options(GenerationOptions::new().with_tools(vec![
            weather_tool()
                .with_description("Get weather")
                .unwrap()
                .with_strict(true),
        ]));
        golden(&encoded_value(&strict, &capabilities), TOOL_STRICT);

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
        golden(&encoded_value(&parallel, &capabilities), PARALLEL_TOOLS);

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
            &encoded_value(&omitted, &capabilities),
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
        golden(&encoded_value(&nested, &capabilities), TOOL_SCHEMA_NESTED);
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
            &encoded_value(&interleaved, &vision_capabilities()),
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
        golden(&encoded_value(&url_only, &vision_capabilities()), IMAGE_URL);

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
        golden(
            &encoded_value(&inline, &vision_capabilities()),
            IMAGE_INLINE,
        );
        let inline_value = encoded_value(&inline, &vision_capabilities());
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
        let default_value = encoded_value(&base, &reasoning_capabilities());
        assert!(default_value.get("reasoning_effort").is_none());

        let high = base.with_options(
            GenerationOptions::new().with_reasoning(ThinkingRequest::Effort(ReasoningEffort::High)),
        );
        golden(
            &encoded_value(&high, &reasoning_capabilities()),
            REASONING_HIGH,
        );

        let disabled = request(vec![Message::user("Hello")])
            .with_options(GenerationOptions::new().with_reasoning(ThinkingRequest::Disabled));
        let disabled_value = encoded_value(&disabled, &reasoning_capabilities());
        assert_eq!(disabled_value["reasoning_effort"], "none");
    }

    #[test]
    fn structured_output_request_goldens_match_and_omit_text_default() {
        let capabilities = structured_capabilities();
        let base = request(vec![Message::user("Return JSON")]);
        assert!(
            encoded_value(&base, &capabilities)
                .get("response_format")
                .is_none()
        );

        let object = base.clone().with_options(
            GenerationOptions::new().with_response_format(ResponseFormat::JsonObject),
        );
        golden(&encoded_value(&object, &capabilities), JSON_OBJECT);

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
            &encoded_value(&schema_request, &capabilities),
            JSON_SCHEMA_STRICT,
        );
    }

    #[test]
    fn image_and_reasoning_capabilities_fail_closed() {
        let image_request = request(vec![Message::new(
            MessageRole::User,
            vec![
                ContentPart::text("look"),
                ContentPart::Image(
                    ImageContent::parse_url("https://example.com/a.png", ImageDetail::Auto)
                        .unwrap(),
                ),
            ],
        )]);
        for status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
            let mut current = vision_capabilities();
            current.vision_input = status;
            assert_capability_error(
                &OpenAiChatRequestAdapter::encode(&image_request, &current),
                "messages.image",
                status,
            );
        }

        let reasoning_request = request(vec![Message::user("Hello")]).with_options(
            GenerationOptions::new().with_reasoning(ThinkingRequest::Effort(ReasoningEffort::Low)),
        );
        assert_capability_error(
            &OpenAiChatRequestAdapter::encode(&reasoning_request, &capabilities()),
            "reasoning",
            CapabilityStatus::Unknown,
        );
    }

    #[test]
    fn tool_capabilities_fail_closed_before_transport() {
        let tools_request = request(vec![Message::user("Hello")])
            .with_options(GenerationOptions::new().with_tools(vec![weather_tool()]));
        for status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
            let mut current = tools_capabilities();
            current.function_tools = status;
            assert_capability_error(
                &OpenAiChatRequestAdapter::encode(&tools_request, &current),
                "tools",
                status,
            );
        }

        let required_request = request(vec![Message::user("Hello")]).with_options(
            GenerationOptions::new()
                .with_tools(vec![weather_tool()])
                .with_tool_choice(ToolChoice::Required),
        );
        let mut current = tools_capabilities();
        current.tool_choice_required = CapabilityStatus::Unknown;
        assert_capability_error(
            &OpenAiChatRequestAdapter::encode(&required_request, &current),
            "tool_choice",
            CapabilityStatus::Unknown,
        );
    }

    #[test]
    fn p1_text_request_remains_unchanged_when_tools_are_absent() {
        golden(
            &encoded_value(&request(vec![Message::user("Hello")]), &capabilities()),
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

        OpenAiChatRequestAdapter::encode(&request, &capabilities()).unwrap();

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
        let profile = OfficialOpenAiProfile::from_api_key(secret)
            .unwrap()
            .profile()
            .unwrap();
        let encoded = OpenAiChatRequestAdapter::encode(
            &request(vec![Message::user("Hello")]),
            profile.capabilities(),
        )
        .unwrap();
        assert!(!String::from_utf8_lossy(&encoded.body).contains(secret));
    }

    fn assert_capability_error<T>(
        result: &Result<T, LlmError>,
        field: &str,
        status: CapabilityStatus,
    ) {
        let expected_state = match status {
            CapabilityStatus::Supported => panic!("supported capability is not an error case"),
            CapabilityStatus::Unsupported => "Unsupported",
            CapabilityStatus::Unknown => "Unknown",
        };
        assert!(matches!(
            result,
            Err(LlmError::Capability(error))
                if error.field() == field && error.state() == expected_state
        ));
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
