use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, header};

use crate::domain::{CapabilityStatus, GenerateRequest, MessageRole};
use crate::error::{CapabilityError, LlmError, ProtocolError};
use crate::provider::ProviderCapabilities;

use super::tool_wire::{encode_parallel_tool_calls, encode_tool_choice, encode_tools};
use super::wire::{ChatCompletionRequestWire, MessageWire};

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
            let [content] = message.content() else {
                return Err(ProtocolError::new(
                    "validated phase-one message did not contain exactly one text part",
                )
                .into());
            };
            messages.push(MessageWire::new(message.role(), content.as_text()));
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

        let wire = ChatCompletionRequestWire::new(
            request.model().model().as_str(),
            messages,
            request.options().temperature(),
            request.options().max_output_tokens(),
            tools,
            tool_choice,
            parallel_tool_calls,
        );
        let body = serde_json::to_vec(&wire).map_err(|_| {
            LlmError::from(ProtocolError::new(
                "failed to serialize validated OpenAI Chat request",
            ))
        })?;

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

    use http::header;
    use serde_json::Value;

    use crate::domain::{
        CapabilityStatus, GenerateRequest, GenerationOptions, Message, ModelRef, ParallelToolCalls,
        ReasoningEffortSupport, RequestMetadata, ToolChoice, ToolDefinition, ToolName, ToolSchema,
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
        let auto_value = encoded_value(&auto, &capabilities);
        assert!(auto_value.get("tool_choice").is_none());

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
        let secret = "sk-canary-p1-011-never-serialize";
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
