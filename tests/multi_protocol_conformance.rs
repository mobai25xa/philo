//! Cross-protocol conformance compares common domain semantics, never wire equality.

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    AssistantMessage, CapabilityStatus, ContentPart, FinishReason, GenerateRequest,
    GenerationOptions, ImageContent, ImageDetail, LlmClient, Message, MessageRole,
    ModelCapabilityProfile, ModelId, ModelRef, ToolDefinition, ToolName, ToolSchema,
};
use serde_json::json;

const ENDPOINT: &str = "http://127.0.0.1:41996/v1/generate";

#[derive(Clone, Copy)]
enum WireProtocol {
    OpenAi,
    Anthropic,
}

fn client(protocol: WireProtocol, response: MockResponse) -> (LlmClient, MockTransport) {
    let capabilities = ModelCapabilityProfile::new(ModelId::new("conformance-model").unwrap())
        .with_function_tools(CapabilityStatus::Supported)
        .with_tool_choice_required(CapabilityStatus::Supported)
        .with_tool_choice_specific(CapabilityStatus::Supported)
        .with_parallel_tool_calls(CapabilityStatus::Supported)
        .with_strict_tools(CapabilityStatus::Supported)
        .with_vision_input(CapabilityStatus::Supported)
        .with_image_detail_original(CapabilityStatus::Unsupported)
        .with_response_format_json_schema(CapabilityStatus::Supported);
    let profile = TestOnlyProfile::localhost(ENDPOINT, "conformance-key")
        .unwrap()
        .with_model_capabilities(capabilities);
    let profile = match protocol {
        WireProtocol::OpenAi => profile,
        WireProtocol::Anthropic => profile.with_anthropic_messages(),
    };
    let transport = MockTransport::scripted([MockExchange::response(response)]);
    (
        LlmClient::new(profile.build().unwrap(), transport.clone()),
        transport,
    )
}

fn request(messages: Vec<Message>) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "conformance-model").unwrap(),
        messages,
    )
    .with_options(GenerationOptions::new().with_max_output_tokens(64))
}

fn sse(body: &'static str) -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(body.as_bytes()))],
    )
}

fn openai_text(finish: &str, text: &str) -> MockResponse {
    let body = format!(
        "data: {{\"id\":\"chatcmpl-common\",\"model\":\"conformance-model\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":{text:?}}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"id\":\"chatcmpl-common\",\"model\":\"conformance-model\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\n\
         data: {{\"id\":\"chatcmpl-common\",\"model\":\"conformance-model\",\"choices\":[],\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}}}\n\n\
         data: [DONE]\n\n"
    );
    owned_sse(body)
}

fn anthropic_text(finish: &str, text: &str) -> MockResponse {
    let body = format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_common\",\"model\":\"conformance-model\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"thinking_tokens\":0}}}}}}\n\n\
         event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{text:?}}}}}\n\n\
         event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{finish}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":1,\"thinking_tokens\":0}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    );
    owned_sse(body)
}

fn owned_sse(body: String) -> MockResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(Bytes::from(body))],
    )
}

fn assert_common_text(left: &AssistantMessage, right: &AssistantMessage) {
    assert_eq!(left.text(), right.text());
    let left_usage = left.usage_details().expect("OpenAI usage details");
    let right_usage = right.usage_details().expect("Anthropic usage details");
    assert_eq!(left_usage.input_tokens(), right_usage.input_tokens());
    assert_eq!(left_usage.output_tokens(), right_usage.output_tokens());
    assert_eq!(left.finish_reason(), right.finish_reason());
}

#[tokio::test]
async fn text_system_and_multiturn_history_have_equivalent_domain_results() {
    let messages = vec![
        Message::system("answer briefly"),
        Message::user("first"),
        Message::assistant("prior"),
        Message::user("second"),
    ];
    let (openai, openai_transport) =
        client(WireProtocol::OpenAi, openai_text("stop", "same answer"));
    let (anthropic, anthropic_transport) = client(
        WireProtocol::Anthropic,
        anthropic_text("end_turn", "same answer"),
    );

    let openai_result = openai.complete(request(messages.clone())).await.unwrap();
    let anthropic_result = anthropic.complete(request(messages)).await.unwrap();
    assert_common_text(&openai_result, &anthropic_result);

    let openai_body: serde_json::Value =
        serde_json::from_slice(openai_transport.captured_requests()[0].body()).unwrap();
    let anthropic_body: serde_json::Value =
        serde_json::from_slice(anthropic_transport.captured_requests()[0].body()).unwrap();
    assert!(openai_body.get("system").is_none());
    assert_eq!(
        anthropic_body["system"],
        json!([{"type": "text", "text": "answer briefly"}])
    );
    assert_ne!(openai_body["messages"], anthropic_body["messages"]);
}

#[tokio::test]
async fn stop_length_and_usage_map_to_the_same_domain_semantics() {
    for (openai_finish, anthropic_finish, expected) in [
        ("stop", "end_turn", FinishReason::Stop),
        ("length", "max_tokens", FinishReason::Length),
    ] {
        let (openai, _) = client(WireProtocol::OpenAi, openai_text(openai_finish, "bounded"));
        let (anthropic, _) = client(
            WireProtocol::Anthropic,
            anthropic_text(anthropic_finish, "bounded"),
        );
        let intent = vec![Message::user("common intent")];
        let left = openai.complete(request(intent.clone())).await.unwrap();
        let right = anthropic.complete(request(intent)).await.unwrap();
        assert_common_text(&left, &right);
        assert_eq!(left.finish_reason(), &expected);
    }
}

#[tokio::test]
async fn one_tool_call_preserves_common_name_arguments_and_finish() {
    let openai = sse(include_str!(
        "fixtures/phase-2/streams/tool-calls/single-call.sse"
    ));
    let anthropic = owned_sse(
        include_str!("fixtures/phase-5/anthropic-messages/stream/tool-use.sse")
            .replace("lookup", "get_weather"),
    );
    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
        "additionalProperties": false
    }))
    .unwrap();
    let options = GenerationOptions::new()
        .with_max_output_tokens(64)
        .with_tools(vec![ToolDefinition::new(
            ToolName::new("get_weather").unwrap(),
            schema,
        )]);
    let intent = || {
        GenerateRequest::new(
            ModelRef::new("test-only", "conformance-model").unwrap(),
            vec![Message::user("weather")],
        )
        .with_options(options.clone())
    };
    let (openai, _) = client(WireProtocol::OpenAi, openai);
    let (anthropic, _) = client(WireProtocol::Anthropic, anthropic);
    let left = openai.complete(intent()).await.unwrap();
    let right = anthropic.complete(intent()).await.unwrap();

    assert_eq!(tool_call(&left).name().as_str(), "get_weather");
    assert_eq!(tool_call(&right).name().as_str(), "get_weather");
    assert_eq!(
        tool_call(&left).arguments().value(),
        &json!({"city": "Paris"})
    );
    assert_eq!(
        tool_call(&right).arguments().value(),
        &json!({"city": "Paris"})
    );
    assert_eq!(left.finish_reason(), &FinishReason::ToolCalls);
    assert_eq!(right.finish_reason(), &FinishReason::ToolCalls);
}

fn tool_call(message: &AssistantMessage) -> &philo::ToolCall {
    match &message.content()[0] {
        ContentPart::ToolCall(call) => call,
        other => panic!("expected tool call, got {other:?}"),
    }
}

#[tokio::test]
async fn common_https_image_input_is_encoded_without_sdk_download() {
    let image =
        ImageContent::parse_url("https://assets.example/image.png", ImageDetail::Auto).unwrap();
    let messages = vec![Message::new(
        MessageRole::User,
        vec![ContentPart::text("inspect"), ContentPart::Image(image)],
    )];
    let (openai, openai_transport) = client(WireProtocol::OpenAi, openai_text("stop", "ok"));
    let (anthropic, anthropic_transport) =
        client(WireProtocol::Anthropic, anthropic_text("end_turn", "ok"));

    let left = openai.complete(request(messages.clone())).await.unwrap();
    let right = anthropic.complete(request(messages)).await.unwrap();
    assert_common_text(&left, &right);
    assert_eq!(openai_transport.captured_requests().len(), 1);
    assert_eq!(anthropic_transport.captured_requests().len(), 1);
}

#[tokio::test]
async fn truncated_streams_fail_closed_for_both_protocols() {
    let (openai, _) = client(
        WireProtocol::OpenAi,
        sse(include_str!("fixtures/responses/openai_chat/truncated.sse")),
    );
    let (anthropic, _) = client(
        WireProtocol::Anthropic,
        sse(include_str!(
            "fixtures/phase-5/anthropic-messages/stream/truncated.sse"
        )),
    );
    for result in [
        openai.complete(request(vec![Message::user("x")])).await,
        anthropic.complete(request(vec![Message::user("x")])).await,
    ] {
        assert!(matches!(result, Err(philo::LlmError::TruncatedStream(_))));
    }
}
