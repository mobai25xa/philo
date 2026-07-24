//! Cross-provider compatibility contracts for the existing official and test-only profiles.

use std::collections::BTreeSet;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    CapabilityStatus, ContentPart, GenerateRequest, GenerationOptions, ImageContent, ImageDetail,
    Message, MessageRole, ModelCapabilityProfile, ModelId, ModelRef, ParallelToolCalls,
    ProtocolDialect, ReasoningEffort, ReasoningEffortSupport, ResponseFormat, StructuredSchema,
    ToolChoice, ToolDefinition, ToolName, ToolSchema,
};
use philo::{
    CompatField, CompatPatch, FinishReasonCompat, MaxOutputTokensWireFormat, PolicySource,
    TokenCount, ToolArgumentsCompat, UsageCompat, resolve_compat,
};
use philo::{OfficialOpenAiProfile, ProviderRuntime};
use serde_json::json;

const OFFICIAL_KEY: &str = "philo-compat-official-key-canary";
const TEST_KEY: &str = "philo-compat-test-key-canary";
const TEST_ENDPOINT: &str = "http://127.0.0.1:41992/v1/chat/completions";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EndpointClass {
    OfficialHttps,
    LoopbackHttp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapabilityExpectations {
    function_tools: CapabilityStatus,
    parallel_tools: CapabilityStatus,
    strict_tools: CapabilityStatus,
    vision_input: CapabilityStatus,
    json_object: CapabilityStatus,
    json_schema: CapabilityStatus,
    reasoning: ReasoningEffortSupport,
}

struct ProviderContractCase {
    name: &'static str,
    expected_provider: &'static str,
    expected_protocol: &'static str,
    model: &'static str,
    endpoint_class: EndpointClass,
    expected_dialect: ProtocolDialect,
    default_capabilities: CapabilityExpectations,
    build: fn() -> ProviderRuntime,
    build_with_capabilities: fn(CapabilityStatus) -> ProviderRuntime,
}

type RequestBuilder = fn(GenerateRequest) -> GenerateRequest;

const DEFAULT_CAPABILITIES: CapabilityExpectations = CapabilityExpectations {
    function_tools: CapabilityStatus::Unknown,
    parallel_tools: CapabilityStatus::Unknown,
    strict_tools: CapabilityStatus::Unknown,
    vision_input: CapabilityStatus::Unknown,
    json_object: CapabilityStatus::Unknown,
    json_schema: CapabilityStatus::Unknown,
    reasoning: ReasoningEffortSupport::Unknown,
};

fn official_runtime() -> ProviderRuntime {
    OfficialOpenAiProfile::from_api_key(OFFICIAL_KEY)
        .unwrap()
        .build()
        .unwrap()
}

fn test_only_runtime() -> ProviderRuntime {
    TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .build()
        .unwrap()
}

fn model_capability_status(status: CapabilityStatus, model: &str) -> ModelCapabilityProfile {
    ModelCapabilityProfile::new(ModelId::new(model).unwrap())
        .with_function_tools(status)
        .with_tool_choice_required(status)
        .with_tool_choice_specific(status)
        .with_parallel_tool_calls(status)
        .with_strict_tools(status)
        .with_vision_input(status)
        .with_image_detail_original(status)
        .with_response_format_json_object(status)
        .with_response_format_json_schema(status)
        .with_reasoning_efforts(match status {
            CapabilityStatus::Supported => ReasoningEffortSupport::Supported(BTreeSet::from([
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::High,
            ])),
            CapabilityStatus::Unsupported => ReasoningEffortSupport::Unsupported,
            CapabilityStatus::Unknown => ReasoningEffortSupport::Unknown,
        })
}

fn official_runtime_with_capabilities(status: CapabilityStatus) -> ProviderRuntime {
    OfficialOpenAiProfile::from_api_key(OFFICIAL_KEY)
        .unwrap()
        .with_model_capabilities(model_capability_status(status, "gpt-compat"))
        .build()
        .unwrap()
}

fn test_only_runtime_with_capabilities(status: CapabilityStatus) -> ProviderRuntime {
    TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .with_model_capabilities(model_capability_status(status, "gpt-test"))
        .build()
        .unwrap()
}

fn cases() -> [ProviderContractCase; 2] {
    [
        ProviderContractCase {
            name: "official-openai",
            expected_provider: "official-openai",
            expected_protocol: "openai-chat-completions",
            model: "gpt-compat",
            endpoint_class: EndpointClass::OfficialHttps,
            expected_dialect: ProtocolDialect::OpenAiChatCompletions,
            default_capabilities: DEFAULT_CAPABILITIES,
            build: official_runtime,
            build_with_capabilities: official_runtime_with_capabilities,
        },
        ProviderContractCase {
            name: "test-only-loopback",
            expected_provider: "test-only",
            expected_protocol: "openai-chat-completions",
            model: "gpt-test",
            endpoint_class: EndpointClass::LoopbackHttp,
            expected_dialect: ProtocolDialect::OpenAiChatCompletions,
            default_capabilities: DEFAULT_CAPABILITIES,
            build: test_only_runtime,
            build_with_capabilities: test_only_runtime_with_capabilities,
        },
    ]
}

#[tokio::test]
async fn typed_request_compat_selects_max_tokens_without_a_public_payload_escape_hatch() {
    let runtime = TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .with_model_compat(
            ModelId::new("gpt-compat-wire").unwrap(),
            CompatPatch::from_source(PolicySource::ModelProfile)
                .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens),
        )
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(success_response_with_text("ok"))]);
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-compat-wire").unwrap(),
        vec![Message::user("hello")],
    )
    .with_options(GenerationOptions::new().with_max_output_tokens(5));
    philo::LlmClient::new(runtime, mock.clone())
        .complete(request)
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(mock.captured_requests()[0].body()).unwrap();
    assert_eq!(body["max_tokens"], 5);
    assert!(body.get("max_completion_tokens").is_none());
}

#[test]
fn compat_merge_is_fieldwise_deterministic_and_traced() {
    let provider = CompatPatch::from_source(PolicySource::ProviderProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens);
    let model = CompatPatch::from_source(PolicySource::ModelProfile)
        .with_tool_arguments(ToolArgumentsCompat::StringOrObject);
    let resolved = resolve_compat(&[provider, model]);
    assert_eq!(
        resolved.request().max_output_tokens,
        MaxOutputTokensWireFormat::MaxTokens
    );
    assert_eq!(
        resolved.response().tool_arguments,
        ToolArgumentsCompat::StringOrObject
    );
    assert_eq!(
        resolved.source(CompatField::RequestMaxOutputTokens),
        PolicySource::ProviderProfile
    );
    assert_eq!(
        resolved.source(CompatField::ResponseToolArguments),
        PolicySource::ModelProfile
    );
    assert_eq!(
        resolved.source(CompatField::RequestImage),
        PolicySource::ProtocolDefault
    );
}

#[tokio::test]
async fn illegal_capability_compat_pairs_fail_before_transport() {
    let runtime = TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .with_model_compat(
            ModelId::new("gpt-invalid-compat").unwrap(),
            CompatPatch::from_source(PolicySource::ModelProfile)
                .with_tool_arguments(ToolArgumentsCompat::StringOrObject),
        )
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-invalid-compat").unwrap(),
        vec![Message::user("hello")],
    );
    let error = philo::LlmClient::new(runtime, mock.clone())
        .complete(request)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("tool argument compatibility"));
    assert!(mock.captured_requests().is_empty());
}

#[tokio::test]
async fn finish_compat_accepts_one_identical_payload_free_repeat_with_usage() {
    let runtime = duplicate_finish_runtime();
    let mock = MockTransport::scripted([MockExchange::response(sse_response(
        br#"data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}

data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{"content":"","reasoning_details":[]},"finish_reason":"stop","native_finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2,"completion_tokens_details":{"reasoning_tokens":2}}}

data: [DONE]

"#,
    ))]);
    let message = philo::LlmClient::new(runtime, mock)
        .complete(duplicate_finish_request())
        .await
        .unwrap();
    assert_eq!(message.finish_reason(), &philo::FinishReason::Stop);
    assert!(message.usage().is_some());
    assert_eq!(
        message.usage_details().unwrap().reasoning_tokens(),
        TokenCount::Unknown
    );
}

#[tokio::test]
async fn strict_usage_compat_rejects_reasoning_larger_than_output() {
    let runtime = TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(sse_response(
        br#"data: {"id":"strict-usage","model":"gpt-test","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2,"completion_tokens_details":{"reasoning_tokens":2}}}

data: [DONE]

"#,
    ))]);
    let error = philo::LlmClient::new(runtime, mock)
        .complete(GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("strict usage")],
        ))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("reasoning tokens exceed output tokens")
    );
}

#[tokio::test]
async fn strict_finish_compat_still_rejects_an_identical_repeat() {
    let runtime = TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .build()
        .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(sse_response(
        br#"data: {"id":"strict-dup","model":"gpt-test","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}

data: {"id":"strict-dup","model":"gpt-test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]

"#,
    ))]);
    let error = philo::LlmClient::new(runtime, mock)
        .complete(GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("strict finish")],
        ))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("duplicate finish reason"));
}

#[tokio::test]
async fn finish_compat_rejects_conflicts_late_reasoning_and_multiple_repeats() {
    let suffixes = [
        r#"data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}

data: [DONE]

"#,
        r#"data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{"reasoning_details":[{"type":"reasoning.text","text":"late"}]},"finish_reason":"stop"}]}

data: [DONE]

"#,
        r#"data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{"future_control":{}},"finish_reason":"stop"}]}

data: [DONE]

"#,
        r#"data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"dup-finish","model":"gpt-duplicate-finish","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]

"#,
    ];
    let expected = [
        "conflicting duplicate finish reason",
        "choice data received after finish reason",
        "choice data received after finish reason",
        "multiple duplicate finish reasons",
    ];
    for (suffix, expected) in suffixes.into_iter().zip(expected) {
        let body = format!(
            "data: {{\"id\":\"dup-finish\",\"model\":\"gpt-duplicate-finish\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"ok\"}},\"finish_reason\":\"stop\"}}]}}\n\n{suffix}"
        );
        let mock = MockTransport::scripted([MockExchange::response(sse_response(body.as_bytes()))]);
        let error = philo::LlmClient::new(duplicate_finish_runtime(), mock)
            .complete(duplicate_finish_request())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

fn duplicate_finish_runtime() -> ProviderRuntime {
    TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .with_model_compat(
            ModelId::new("gpt-duplicate-finish").unwrap(),
            CompatPatch::from_source(PolicySource::ModelProfile)
                .with_finish_reason(FinishReasonCompat::AllowOneIdenticalDuplicate)
                .with_usage(UsageCompat::OpenAiDropInconsistentReasoning),
        )
        .build()
        .unwrap()
}

fn duplicate_finish_request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-duplicate-finish").unwrap(),
        vec![Message::user("duplicate finish compatibility")],
    )
}

fn sse_response(body: impl AsRef<[u8]>) -> MockResponse {
    MockResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        )]),
        vec![MockBodyItem::chunk(Bytes::copy_from_slice(body.as_ref()))],
    )
}

#[tokio::test]
async fn typed_response_compat_normalizes_object_tool_arguments_in_the_private_adapter() {
    let runtime = TestOnlyProfile::localhost(TEST_ENDPOINT, TEST_KEY)
        .unwrap()
        .with_model_capabilities(model_capability_status(
            CapabilityStatus::Supported,
            "gpt-object",
        ))
        .with_model_compat(
            ModelId::new("gpt-object").unwrap(),
            CompatPatch::from_source(PolicySource::ModelProfile)
                .with_tool_arguments(ToolArgumentsCompat::StringOrObject),
        )
        .build()
        .unwrap();
    let response = MockResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))]),
        vec![MockBodyItem::chunk(Bytes::from_static(
            br#"data: {"id":"object-tool","model":"gpt-object","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":{"city":"Paris"}}}]},"finish_reason":null}]}

data: {"id":"object-tool","model":"gpt-object","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
        ))],
    );
    let mock = MockTransport::scripted([MockExchange::response(response)]);
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-object").unwrap(),
        vec![Message::user("lookup")],
    )
    .with_options(GenerationOptions::new().with_tools(vec![tool()]));
    let message = philo::LlmClient::new(runtime, mock)
        .complete(request)
        .await
        .unwrap();
    let tool_call = message
        .content()
        .iter()
        .find_map(|part| match part {
            ContentPart::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("object tool call should be normalized");
    assert_eq!(tool_call.arguments().raw_json(), r#"{"city":"Paris"}"#);
}

fn assert_capabilities(
    case: &ProviderContractCase,
    actual: &philo::ProviderCapabilities,
    expected: &CapabilityExpectations,
) {
    assert_eq!(
        actual.function_tools, expected.function_tools,
        "{} function tools",
        case.name
    );
    assert_eq!(
        actual.parallel_tool_calls, expected.parallel_tools,
        "{} parallel tools",
        case.name
    );
    assert_eq!(
        actual.strict_tools, expected.strict_tools,
        "{} strict tools",
        case.name
    );
    assert_eq!(
        actual.vision_input, expected.vision_input,
        "{} vision",
        case.name
    );
    assert_eq!(
        actual.response_format_json_object, expected.json_object,
        "{} JSON object",
        case.name
    );
    assert_eq!(
        actual.response_format_json_schema, expected.json_schema,
        "{} JSON schema",
        case.name
    );
    assert_eq!(
        &actual.reasoning_efforts, &expected.reasoning,
        "{} reasoning",
        case.name
    );
}

fn request(case: &ProviderContractCase) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new(case.expected_provider, case.model).unwrap(),
        vec![Message::user("compatibility harness request")],
    )
}

fn tool() -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new("compat_tool").unwrap(),
        ToolSchema::new(json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        }))
        .unwrap(),
    )
}

fn success_response_with_text(text: &str) -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    let delta = json!({
        "id": "compat-generation",
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "delta": {"content": text},
            "finish_reason": null
        }]
    });
    let finish = json!({
        "id": "compat-generation",
        "model": "gpt-test",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from(format!(
            "data: {delta}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
        )))],
    )
}

fn success_response() -> MockResponse {
    success_response_with_text("ok")
}

#[test]
fn official_and_test_only_profiles_compile_to_typed_runtime() {
    for case in cases() {
        let runtime = (case.build)();
        assert_eq!(
            runtime.provider_id().as_str(),
            case.expected_provider,
            "{}",
            case.name
        );
        assert_eq!(
            runtime.protocol_id().as_str(),
            case.expected_protocol,
            "{}",
            case.name
        );
        assert_eq!(runtime.dialect(), case.expected_dialect, "{}", case.name);
        assert_eq!(runtime.method(), http::Method::POST);
        match case.endpoint_class {
            EndpointClass::OfficialHttps => {
                assert_eq!(runtime.endpoint().url().scheme(), "https");
                assert_eq!(runtime.endpoint().url().host_str(), Some("api.openai.com"));
            }
            EndpointClass::LoopbackHttp => {
                assert_eq!(runtime.endpoint().url().scheme(), "http");
                assert_eq!(runtime.endpoint().url().host_str(), Some("127.0.0.1"));
            }
        }
        assert!(
            !format!("{runtime:?}").contains("canary"),
            "{} debug leaked a secret",
            case.name
        );
    }
}

#[test]
fn profile_cases_resolve_explicit_protocol_and_dialect() {
    for case in cases() {
        let runtime = (case.build)();
        assert_eq!(runtime.protocol_id().as_str(), "openai-chat-completions");
        assert_eq!(runtime.dialect(), ProtocolDialect::OpenAiChatCompletions);
        assert_capabilities(
            &case,
            &runtime.capabilities_for(&ModelId::new(case.model).unwrap()),
            &case.default_capabilities,
        );
    }
}

#[test]
fn exact_model_capabilities_override_defaults_without_brand_branches() {
    for case in cases() {
        let runtime = (case.build_with_capabilities)(CapabilityStatus::Supported);
        let capabilities = runtime.capabilities_for(&ModelId::new(case.model).unwrap());
        let expected = CapabilityExpectations {
            function_tools: CapabilityStatus::Supported,
            parallel_tools: CapabilityStatus::Supported,
            strict_tools: CapabilityStatus::Supported,
            vision_input: CapabilityStatus::Supported,
            json_object: CapabilityStatus::Supported,
            json_schema: CapabilityStatus::Supported,
            reasoning: ReasoningEffortSupport::Supported(BTreeSet::from([
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::High,
            ])),
        };
        assert_capabilities(&case, &capabilities, &expected);
    }
}

#[tokio::test]
async fn supported_capabilities_use_the_shared_protocol_wire_for_every_case() {
    for case in cases() {
        let schema =
            StructuredSchema::new("compat_schema", None, tool().parameters().clone(), true)
                .unwrap();
        let image =
            ImageContent::parse_url("https://example.com/compat.png", ImageDetail::Original)
                .unwrap();
        let advanced = GenerateRequest::new(
            ModelRef::new(case.expected_provider, case.model).unwrap(),
            vec![Message::new(
                MessageRole::User,
                vec![ContentPart::text("inspect"), ContentPart::Image(image)],
            )],
        )
        .with_options(
            GenerationOptions::new()
                .with_tools(vec![tool().with_strict(true)])
                .with_tool_choice(ToolChoice::Required)
                .with_parallel_tool_calls(ParallelToolCalls::Enabled)
                .with_response_format(ResponseFormat::JsonSchema(schema))
                .with_reasoning(philo::ThinkingRequest::Effort(ReasoningEffort::High)),
        );
        let mock = MockTransport::scripted([MockExchange::response(success_response_with_text(
            r#"{"value":"ok"}"#,
        ))]);
        let client = philo::LlmClient::new(
            (case.build_with_capabilities)(CapabilityStatus::Supported),
            mock.clone(),
        );
        client.complete(advanced).await.unwrap();
        mock.assert_consumed();
        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1, "{} request count", case.name);
        let body: serde_json::Value = serde_json::from_slice(captured[0].body()).unwrap();
        assert_eq!(body["stream"], true, "{} streaming", case.name);
        assert_eq!(
            body["tools"][0]["function"]["strict"], true,
            "{} strict",
            case.name
        );
        assert_eq!(body["tool_choice"], "required", "{} tool choice", case.name);
        assert_eq!(body["parallel_tool_calls"], true, "{} parallel", case.name);
        assert_eq!(
            body["response_format"]["type"], "json_schema",
            "{} schema",
            case.name
        );
        assert_eq!(body["reasoning_effort"], "high", "{} reasoning", case.name);
        assert_eq!(
            body["messages"][0]["content"][1]["type"], "image_url",
            "{} vision",
            case.name
        );
    }
}

#[tokio::test]
async fn unsupported_capabilities_fail_before_transport_for_every_case() {
    let requests: [(&str, RequestBuilder); 7] = [
        ("function tools", |request| {
            request.with_options(GenerationOptions::new().with_tools(vec![tool()]))
        }),
        ("parallel tools", |request| {
            request.with_options(
                GenerationOptions::new()
                    .with_tools(vec![tool()])
                    .with_parallel_tool_calls(ParallelToolCalls::Enabled),
            )
        }),
        ("strict tools", |request| {
            request
                .with_options(GenerationOptions::new().with_tools(vec![tool().with_strict(true)]))
        }),
        ("vision", |request| {
            GenerateRequest::new(
                request.model().clone(),
                vec![Message::new(
                    MessageRole::User,
                    vec![
                        ContentPart::text("inspect"),
                        ContentPart::Image(
                            ImageContent::parse_url(
                                "https://example.com/compat.png",
                                ImageDetail::Auto,
                            )
                            .unwrap(),
                        ),
                    ],
                )],
            )
        }),
        ("JSON object", |request| {
            request.with_options(
                GenerationOptions::new().with_response_format(ResponseFormat::JsonObject),
            )
        }),
        ("JSON schema", |request| {
            let schema =
                StructuredSchema::new("compat_schema", None, tool().parameters().clone(), true)
                    .unwrap();
            request.with_options(
                GenerationOptions::new().with_response_format(ResponseFormat::JsonSchema(schema)),
            )
        }),
        ("reasoning", |request| {
            request.with_options(
                GenerationOptions::new()
                    .with_reasoning(philo::ThinkingRequest::Effort(ReasoningEffort::High)),
            )
        }),
    ];
    for case in cases() {
        for (capability, build_request) in requests {
            let mock = MockTransport::default();
            let client = philo::LlmClient::new((case.build)(), mock.clone());
            let error = client
                .complete(build_request(request(&case)))
                .await
                .unwrap_err();
            assert!(
                !format!("{error:?}").contains("canary"),
                "{case_name}: {capability}",
                case_name = case.name
            );
            assert!(
                mock.captured_requests().is_empty(),
                "{case_name}: {capability} reached transport",
                case_name = case.name
            );
        }
    }
}

#[test]
fn profile_cases_preserve_header_auth_and_audience_security() {
    for case in cases() {
        let runtime = (case.build)();
        assert_eq!(
            runtime.transport_options().redirect_policy(),
            philo::RedirectPolicy::Disabled
        );
        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            "x-compat-request",
            HeaderValue::from_static("request-value"),
        );
        let resolved = runtime
            .resolve_headers(Vec::new(), &request_headers)
            .unwrap();
        assert_eq!(resolved.headers()["content-type"], "application/json");
        assert_eq!(resolved.headers()["x-compat-request"], "request-value");
        assert!(
            resolved.headers()[header::AUTHORIZATION]
                .to_str()
                .unwrap()
                .contains("canary")
        );

        let mut protected_override = HeaderMap::new();
        protected_override.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer attacker"),
        );
        assert!(
            runtime
                .resolve_headers(Vec::new(), &protected_override)
                .is_err(),
            "{} accepted auth override",
            case.name
        );
    }
}

#[tokio::test]
async fn provider_requests_share_protocol_pipeline_without_header_crosstalk() {
    for case in cases() {
        let mock = MockTransport::scripted([
            MockExchange::response(success_response()),
            MockExchange::response(success_response()),
        ]);
        let client = philo::LlmClient::new((case.build)(), mock.clone());
        let first = request(&case);
        let second = request(&case);
        let (first_result, second_result) =
            tokio::join!(client.complete(first), client.complete(second));
        first_result.unwrap();
        second_result.unwrap();
        mock.assert_consumed();
        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 2, "{} request count", case.name);
        assert_ne!(
            captured[0].local_request_id(),
            captured[1].local_request_id(),
            "{} local IDs crossed",
            case.name
        );
        for captured in captured {
            let debug = format!("{captured:?}");
            assert!(!debug.contains(OFFICIAL_KEY));
            assert!(!debug.contains(TEST_KEY));
            assert!(!debug.contains("compatibility harness request"));
            assert_eq!(captured.method(), &http::Method::POST);
            assert_eq!(captured.headers()[header::CONTENT_TYPE], "application/json");
            assert_eq!(captured.headers()[header::ACCEPT], "text/event-stream");
            assert!(
                captured.headers()[header::AUTHORIZATION]
                    .to_str()
                    .unwrap()
                    .contains("canary")
            );
            let body: serde_json::Value = serde_json::from_slice(captured.body()).unwrap();
            assert_eq!(body["stream"], true);
            assert_eq!(
                body["messages"][0]["content"],
                "compatibility harness request"
            );
        }
    }
}

#[test]
fn provider_contract_debug_is_value_free() {
    for case in cases() {
        let runtime = (case.build)();
        let debug = format!("{runtime:?}");
        assert!(!debug.contains(OFFICIAL_KEY));
        assert!(!debug.contains(TEST_KEY));
        assert!(!debug.contains("Authorization"));
        assert!(!debug.contains("request-value"));
    }
}
