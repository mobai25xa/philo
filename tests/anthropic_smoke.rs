//! Opt-in, value-free controlled smoke for the official Anthropic Messages target.

use futures_util::StreamExt as _;
use philo::domain::ids::ToolName;
use philo::domain::request::CapabilityStatus;
use philo::domain::schema::ToolSchema;
use philo::domain::tools::{ToolChoice, ToolDefinition};
use philo::protocol_options::{AnthropicMessagesOptions, AnthropicThinkingDisplay};
use philo::provider::capability::ModelCapabilityProfile;
use philo::provider::profiles::OfficialAnthropicProfile;
use philo::{
    AssistantEvent, FinishReason, GenerateRequest, GenerationOptions, LlmClient, Message, ModelId,
    ModelRef, RequestControl,
};
use serde_json::json;

fn request(model: &str, prompt: &str, options: GenerationOptions) -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("official-anthropic", model).unwrap(),
        vec![Message::user(prompt)],
    )
    .with_options(options)
}

fn client() -> (LlmClient, String) {
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY is required");
    let model = std::env::var("ANTHROPIC_MODEL").expect("ANTHROPIC_MODEL is required");
    let capabilities = ModelCapabilityProfile::new(ModelId::new(&model).unwrap())
        .with_function_tools(CapabilityStatus::Supported)
        .with_tool_choice_required(CapabilityStatus::Supported)
        .with_tool_choice_specific(CapabilityStatus::Supported)
        .with_parallel_tool_calls(CapabilityStatus::Supported)
        .with_strict_tools(CapabilityStatus::Supported)
        .with_vision_input(CapabilityStatus::Supported)
        .with_image_detail_original(CapabilityStatus::Unsupported)
        .with_response_format_json_schema(CapabilityStatus::Supported)
        .with_adaptive_thinking(CapabilityStatus::Supported)
        .with_adaptive_thinking_effort(CapabilityStatus::Supported);
    let runtime = OfficialAnthropicProfile::from_api_key(key)
        .unwrap()
        .with_model_capabilities(capabilities)
        .build()
        .unwrap();
    (LlmClient::with_reqwest(runtime).unwrap(), model)
}

#[tokio::test]
#[ignore = "requires an explicitly approved, quota-limited Anthropic credential"]
#[allow(clippy::too_many_lines)]
async fn anthropic_controlled_smoke() {
    let (client, model) = client();

    let mut stream = client
        .stream(request(
            &model,
            "Reply with one short word.",
            GenerationOptions::new().with_max_output_tokens(32),
        ))
        .await
        .unwrap();
    let mut saw_text = false;
    let mut saw_usage = false;
    let mut saw_request_id = false;
    let mut saw_done = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            AssistantEvent::Start {
                provider_request_id,
                ..
            } => saw_request_id = provider_request_id.is_some(),
            AssistantEvent::TextDelta { .. } => saw_text = true,
            AssistantEvent::Usage(_) | AssistantEvent::DetailedUsage(_) => saw_usage = true,
            AssistantEvent::Done { finish_reason } => {
                assert!(matches!(
                    finish_reason,
                    FinishReason::Stop | FinishReason::Length
                ));
                saw_done = true;
            }
            _ => {}
        }
    }
    assert!(saw_text && saw_usage && saw_request_id && saw_done);

    let schema = ToolSchema::new(json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "required": ["value"],
        "additionalProperties": false
    }))
    .unwrap();
    let tool_options = GenerationOptions::new()
        .with_max_output_tokens(64)
        .with_tools(vec![ToolDefinition::new(
            ToolName::new("echo_value").unwrap(),
            schema,
        )])
        .with_tool_choice(ToolChoice::Required);
    let tool_message = client
        .complete(request(
            &model,
            "Call the declared tool with a short harmless value.",
            tool_options,
        ))
        .await
        .unwrap();
    assert_eq!(tool_message.finish_reason(), &FinishReason::ToolCalls);
    assert!(
        tool_message
            .content()
            .iter()
            .any(|part| matches!(part, philo::ContentPart::ToolCall(_)))
    );

    let thinking_options = GenerationOptions::new()
        .with_max_output_tokens(128)
        .with_protocol_options(
            AnthropicMessagesOptions::new()
                .with_adaptive_thinking(AnthropicThinkingDisplay::Omitted),
        );
    let thinking = client
        .complete(request(
            &model,
            "Return a short answer after reasoning.",
            thinking_options,
        ))
        .await
        .unwrap();
    assert!(!thinking.text().is_empty());

    let invalid = request(
        "philo-controlled-invalid-model",
        "This request must fail before producing content.",
        GenerationOptions::new().with_max_output_tokens(1),
    );
    let error = client.complete(invalid).await.unwrap_err();
    assert!(
        matches!(error, philo::LlmError::HttpStatus(ref error) if (400..500).contains(&error.status()))
    );

    let control = RequestControl::new();
    let cancellation = control.cancellation_token().clone();
    let mut cancellable = client
        .stream_with_control(
            request(
                &model,
                "Begin a short response.",
                GenerationOptions::new().with_max_output_tokens(64),
            ),
            control,
        )
        .await
        .unwrap();
    let _ = cancellable.next().await;
    cancellation.cancel();
    drop(cancellable);
    assert!(cancellation.is_cancelled());
}

#[test]
fn smoke_source_is_value_free_and_explicitly_ignored() {
    let source = include_str!("anthropic_smoke.rs");
    let forbidden = [
        ["print", "ln!"].concat(),
        ["eprint", "ln!"].concat(),
        ["d", "bg!"].concat(),
        ["sk", "-ant-"].concat(),
    ];
    for forbidden in forbidden {
        assert!(
            !source.contains(&forbidden),
            "smoke source contains {forbidden}"
        );
    }
    assert!(source.contains("#[ignore ="));
    assert!(source.contains("ANTHROPIC_API_KEY"));
    assert!(source.contains("ANTHROPIC_MODEL"));
}
