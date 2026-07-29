//! Domain event collection and exact-model capability contracts.

mod support;

use std::collections::BTreeSet;

use bytes::Bytes;
use futures_util::stream;
use philo::domain::content::{
    ImageMime, ImageSource, OpaqueReasoning, SourceIdentity, ThinkingContent,
};
use philo::domain::event::collect_assistant_message;
use philo::domain::ids::{
    ContentIndex, LocalRequestId, ProtocolId, ToolCallId, ToolName, WireToolIndex,
};
use philo::domain::request::{
    CapabilitySet, CapabilityStatus, ReasoningEffort, ReasoningEffortSupport,
};
use philo::domain::tools::{ToolArguments, ToolCall};
use philo::provider::capability::ModelCapabilityProfile;
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::{AssistantEvent, ContentPart, FinishReason, LlmError, ModelId, ProviderId, Usage};

fn index(value: u32) -> ContentIndex {
    ContentIndex::new(value)
}

fn wire_index(value: u32) -> WireToolIndex {
    WireToolIndex::new(value)
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall::new(
        ToolCallId::new(id).unwrap(),
        ToolName::new(name).unwrap(),
        ToolArguments::from_raw_json(arguments).unwrap(),
    )
}

#[test]
fn ids_and_completed_tool_arguments_enforce_frozen_boundaries() {
    assert_eq!(index(7).get(), 7);
    assert_eq!(wire_index(9).get(), 9);

    assert!(ToolCallId::new("").is_err());
    assert!(ToolCallId::new("x".repeat(257)).is_err());
    assert_eq!(
        ToolCallId::new(" provider id ").unwrap().as_str(),
        " provider id "
    );

    for invalid in ["", "has space", "bad/slash", "工具"] {
        assert!(ToolName::new(invalid).is_err());
    }
    assert!(ToolName::new("x".repeat(65)).is_err());
    assert_eq!(
        ToolName::new("lookup-v2_1").unwrap().as_str(),
        "lookup-v2_1"
    );

    assert!(ToolArguments::from_raw_json(r#"{"incomplete":true"#).is_err());
    let arguments = ToolArguments::from_raw_json(r#" { "secret": "argument-canary" } "#).unwrap();
    assert_eq!(arguments.value()["secret"], "argument-canary");
    assert_eq!(arguments.raw_json(), r#" { "secret": "argument-canary" } "#);
    let debug = format!("{arguments:?}");
    assert!(!debug.contains("argument-canary"));
    assert!(!debug.contains("secret"));
}

#[test]
fn opaque_reasoning_and_visible_thinking_are_redacted_in_debug() {
    let source = SourceIdentity::new(
        ProviderId::new("provider").unwrap(),
        ModelId::new("model").unwrap(),
        ProtocolId::new("protocol").unwrap(),
    );
    let opaque = OpaqueReasoning::new(
        Bytes::from_static(b"opaque-reasoning-canary"),
        source,
        false,
    );
    let thinking = ThinkingContent::new("visible-thinking-canary").with_opaque(opaque);

    let debug = format!("{:?}", ContentPart::Thinking(thinking));
    assert!(!debug.contains("opaque-reasoning-canary"));
    assert!(!debug.contains("visible-thinking-canary"));
    assert!(debug.contains("text_bytes"));
    assert!(debug.contains("provider"));
}

#[test]
fn image_source_debug_does_not_expose_urls_or_payloads() {
    let url = ImageSource::Url(
        url::Url::parse("https://example.com/image.png?token=image-query-canary").unwrap(),
    );
    let inline = ImageSource::Inline {
        mime: ImageMime::Png,
        bytes: Bytes::from_static(b"inline-image-canary"),
    };
    let data_url = ImageSource::DataUrl("data:image/png;base64,data-url-canary".to_owned());

    let debug = format!("{url:?} {inline:?} {data_url:?}");
    assert!(!debug.contains("image-query-canary"));
    assert!(!debug.contains("inline-image-canary"));
    assert!(!debug.contains("data-url-canary"));
    assert!(debug.contains("has_query"));
}

#[tokio::test]
async fn collector_preserves_interleaving_and_parallel_tool_call_identity() {
    let first = call("call-first", "first_tool", r#"{"a":1}"#);
    let second = call("call-second", "second_tool", r#"{"b":2}"#);
    let events = vec![
        Ok(AssistantEvent::start(
            LocalRequestId::new("local-p2").unwrap(),
        )),
        Ok(AssistantEvent::TextStart { index: index(0) }),
        Ok(AssistantEvent::TextDelta {
            index: index(0),
            delta: "A".to_owned(),
        }),
        Ok(AssistantEvent::ToolCallStart {
            index: index(1),
            wire_index: wire_index(0),
            id: Some(ToolCallId::new("call-first").unwrap()),
        }),
        Ok(AssistantEvent::ToolCallStart {
            index: index(2),
            wire_index: wire_index(1),
            id: None,
        }),
        Ok(AssistantEvent::ToolCallDelta {
            index: index(2),
            wire_index: wire_index(1),
            name_delta: Some("second_".to_owned()),
            arguments_delta: Some(r#"{"b":"#.to_owned()),
        }),
        Ok(AssistantEvent::TextDelta {
            index: index(0),
            delta: "B".to_owned(),
        }),
        Ok(AssistantEvent::ToolCallDelta {
            index: index(1),
            wire_index: wire_index(0),
            name_delta: Some("first_tool".to_owned()),
            arguments_delta: Some(r#"{"a":1}"#.to_owned()),
        }),
        Ok(AssistantEvent::ToolCallDelta {
            index: index(2),
            wire_index: wire_index(1),
            name_delta: Some("tool".to_owned()),
            arguments_delta: Some("2}".to_owned()),
        }),
        Ok(AssistantEvent::ToolCallEnd {
            index: index(2),
            call: second.clone(),
        }),
        Ok(AssistantEvent::TextEnd { index: index(0) }),
        Ok(AssistantEvent::ToolCallEnd {
            index: index(1),
            call: first.clone(),
        }),
        Ok(AssistantEvent::Usage(Usage::new(4, 3, 7).unwrap())),
        Ok(AssistantEvent::Done {
            finish_reason: FinishReason::ToolCalls,
        }),
    ];

    let message = collect_assistant_message(stream::iter(events))
        .await
        .unwrap();
    assert_eq!(message.text(), "AB");
    assert_eq!(message.content().len(), 3);
    assert_eq!(message.content()[0], ContentPart::text("AB"));
    assert_eq!(message.content()[1], ContentPart::ToolCall(first));
    assert_eq!(message.content()[2], ContentPart::ToolCall(second));
    assert_eq!(message.usage().unwrap().total_tokens(), 7);
    assert_eq!(message.finish_reason(), &FinishReason::ToolCalls);
}

#[tokio::test]
async fn collector_keeps_thinking_separate_from_text() {
    let events = vec![
        Ok(AssistantEvent::TextStart { index: index(0) }),
        Ok(AssistantEvent::ThinkingStart { index: index(1) }),
        Ok(AssistantEvent::ThinkingDelta {
            index: index(1),
            delta: "reason".to_owned(),
        }),
        Ok(AssistantEvent::TextDelta {
            index: index(0),
            delta: "answer".to_owned(),
        }),
        Ok(AssistantEvent::ThinkingEnd { index: index(1) }),
        Ok(AssistantEvent::TextEnd { index: index(0) }),
        Ok(AssistantEvent::Done {
            finish_reason: FinishReason::Stop,
        }),
    ];

    let message = collect_assistant_message(stream::iter(events))
        .await
        .unwrap();
    assert_eq!(message.text(), "answer");
    assert_eq!(message.content()[0], ContentPart::text("answer"));
    let ContentPart::Thinking(thinking) = &message.content()[1] else {
        panic!("expected thinking content");
    };
    assert_eq!(thinking.text(), "reason");
    assert!(thinking.opaque().is_none());
}

#[tokio::test]
async fn collector_rejects_unknown_duplicate_and_incomplete_blocks() {
    let missing_start = vec![Ok(AssistantEvent::TextDelta {
        index: index(0),
        delta: "x".to_owned(),
    })];
    assert!(matches!(
        collect_assistant_message(stream::iter(missing_start)).await,
        Err(LlmError::Protocol(_))
    ));

    let unknown_index = vec![Ok(AssistantEvent::TextStart { index: index(1) })];
    assert!(matches!(
        collect_assistant_message(stream::iter(unknown_index)).await,
        Err(LlmError::Protocol(_))
    ));

    let duplicate_end = vec![
        Ok(AssistantEvent::TextStart { index: index(0) }),
        Ok(AssistantEvent::TextEnd { index: index(0) }),
        Ok(AssistantEvent::TextEnd { index: index(0) }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(duplicate_end)).await,
        Err(LlmError::Protocol(_))
    ));

    let incomplete_call = vec![
        Ok(AssistantEvent::ToolCallStart {
            index: index(0),
            wire_index: wire_index(0),
            id: None,
        }),
        Ok(AssistantEvent::Done {
            finish_reason: FinishReason::ToolCalls,
        }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(incomplete_call)).await,
        Err(LlmError::Protocol(_))
    ));

    let truncated = vec![
        Ok(AssistantEvent::TextStart { index: index(0) }),
        Ok(AssistantEvent::TextDelta {
            index: index(0),
            delta: "partial".to_owned(),
        }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(truncated)).await,
        Err(LlmError::TruncatedStream(_))
    ));
}

#[tokio::test]
async fn tool_call_end_must_match_the_accumulated_identity_and_json() {
    let events = vec![
        Ok(AssistantEvent::ToolCallStart {
            index: index(0),
            wire_index: wire_index(3),
            id: Some(ToolCallId::new("original-id").unwrap()),
        }),
        Ok(AssistantEvent::ToolCallDelta {
            index: index(0),
            wire_index: wire_index(3),
            name_delta: Some("lookup".to_owned()),
            arguments_delta: Some("{}".to_owned()),
        }),
        Ok(AssistantEvent::ToolCallEnd {
            index: index(0),
            call: call("changed-id", "lookup", "{}"),
        }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(events)).await,
        Err(LlmError::Protocol(_))
    ));
}

#[tokio::test]
async fn collector_rejects_duplicate_tool_wire_indexes_and_ids() {
    let duplicate_wire = vec![
        Ok(AssistantEvent::ToolCallStart {
            index: index(0),
            wire_index: wire_index(0),
            id: Some(ToolCallId::new("first").unwrap()),
        }),
        Ok(AssistantEvent::ToolCallStart {
            index: index(1),
            wire_index: wire_index(0),
            id: Some(ToolCallId::new("second").unwrap()),
        }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(duplicate_wire)).await,
        Err(LlmError::Protocol(_))
    ));

    let duplicate_id = vec![
        Ok(AssistantEvent::ToolCallStart {
            index: index(0),
            wire_index: wire_index(0),
            id: Some(ToolCallId::new("same").unwrap()),
        }),
        Ok(AssistantEvent::ToolCallStart {
            index: index(1),
            wire_index: wire_index(1),
            id: Some(ToolCallId::new("same").unwrap()),
        }),
    ];
    assert!(matches!(
        collect_assistant_message(stream::iter(duplicate_id)).await,
        Err(LlmError::Protocol(_))
    ));
}

#[test]
fn extended_capability_defaults_preserve_core_defaults() {
    let capabilities = CapabilitySet::default();
    assert_eq!(capabilities.temperature, CapabilityStatus::Supported);
    assert_eq!(capabilities.max_output_tokens, CapabilityStatus::Supported);
    for status in [
        capabilities.function_tools,
        capabilities.tool_choice_required,
        capabilities.tool_choice_specific,
        capabilities.parallel_tool_calls,
        capabilities.strict_tools,
        capabilities.vision_input,
        capabilities.image_detail_original,
        capabilities.response_format_json_object,
        capabilities.response_format_json_schema,
    ] {
        assert_eq!(status, CapabilityStatus::Unknown);
    }
    assert_eq!(
        capabilities.reasoning_efforts,
        ReasoningEffortSupport::Unknown
    );
}

#[test]
fn exact_model_profile_overrides_only_declared_model_capabilities() {
    let model = ModelId::new("gpt-exact").unwrap();
    let efforts = BTreeSet::from([ReasoningEffort::Low, ReasoningEffort::High]);
    let first = ModelCapabilityProfile::new(model.clone())
        .with_function_tools(CapabilityStatus::Unsupported);
    let replacement = ModelCapabilityProfile::new(model.clone())
        .with_function_tools(CapabilityStatus::Supported)
        .with_tool_choice_required(CapabilityStatus::Supported)
        .with_tool_choice_specific(CapabilityStatus::Unsupported)
        .with_parallel_tool_calls(CapabilityStatus::Supported)
        .with_strict_tools(CapabilityStatus::Supported)
        .with_vision_input(CapabilityStatus::Supported)
        .with_image_detail_original(CapabilityStatus::Unsupported)
        .with_response_format_json_object(CapabilityStatus::Supported)
        .with_response_format_json_schema(CapabilityStatus::Supported)
        .with_reasoning_efforts(ReasoningEffortSupport::Supported(efforts.clone()));

    let runtime = OfficialOpenAiProfile::from_api_key("test-key")
        .unwrap()
        .with_model_capabilities(first)
        .with_model_capabilities(replacement)
        .build()
        .unwrap();

    assert!(std::ptr::eq(runtime.capabilities(), runtime.capabilities()));
    assert_eq!(
        runtime.capabilities().function_tools,
        CapabilityStatus::Unknown
    );
    let exact = runtime.capabilities_for(&model);
    assert_eq!(exact.function_tools, CapabilityStatus::Supported);
    assert_eq!(exact.tool_choice_required, CapabilityStatus::Supported);
    assert_eq!(exact.tool_choice_specific, CapabilityStatus::Unsupported);
    assert_eq!(exact.parallel_tool_calls, CapabilityStatus::Supported);
    assert_eq!(exact.strict_tools, CapabilityStatus::Supported);
    assert_eq!(exact.vision_input, CapabilityStatus::Supported);
    assert_eq!(exact.image_detail_original, CapabilityStatus::Unsupported);
    assert_eq!(
        exact.reasoning_efforts,
        ReasoningEffortSupport::Supported(efforts)
    );
    assert_eq!(exact.temperature, CapabilityStatus::Supported);
    assert_eq!(exact.streaming, CapabilityStatus::Supported);

    for non_exact in ["gpt-exact-2026", "GPT-EXACT", "gpt"] {
        let unresolved = runtime.capabilities_for(&ModelId::new(non_exact).unwrap());
        assert_eq!(unresolved.function_tools, CapabilityStatus::Unknown);
        assert_eq!(
            unresolved.reasoning_efforts,
            ReasoningEffortSupport::Unknown
        );
    }

    let mut detached = runtime.capabilities_for(&model);
    detached.function_tools = CapabilityStatus::Unsupported;
    assert_eq!(
        runtime.capabilities_for(&model).function_tools,
        CapabilityStatus::Supported
    );

    let test_model = ModelId::new("test-exact").unwrap();
    let test_runtime = support::provider::TestProvider::new(
        "https://test.invalid/v1/chat/completions",
        "test-key",
    )
    .unwrap()
    .with_model_capabilities(
        ModelCapabilityProfile::new(test_model.clone())
            .with_function_tools(CapabilityStatus::Supported),
    )
    .build()
    .unwrap();
    assert_eq!(
        test_runtime.capabilities_for(&test_model).function_tools,
        CapabilityStatus::Supported
    );
}
