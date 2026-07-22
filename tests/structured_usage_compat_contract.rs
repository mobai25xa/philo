//! Phase-two structured output, usage/cost, and dialect combination contracts.

use std::collections::BTreeSet;

use futures_util::stream;
use philo::{
    AssistantEvent, CapabilitySet, CapabilityStatus, ContentIndex, ContentPart, CostFailure,
    CurrencyCode, DialectPolicy, FinishReason, GenerateRequest, GenerationOptions, LlmError,
    LocalRequestId, Message, ModelRef, MoneyAmount, PriceProfile, ReasoningEffort,
    ReasoningEffortSupport, ResponseFormat, StructuredOutputFailure, StructuredSchema, TokenCount,
    ToolSchema, Usage, UsageDetails, UsageMergeOutcome, collect_assistant_message,
    collect_assistant_message_for_format, estimate_cost, merge_usage_details,
};
use serde_json::json;

fn text_events(text: &str, finish: FinishReason) -> Vec<Result<AssistantEvent, LlmError>> {
    vec![
        Ok(AssistantEvent::Start {
            local_request_id: LocalRequestId::new("local-p2").unwrap(),
            provider_request_id: None,
            generation_id: None,
        }),
        Ok(AssistantEvent::TextStart {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::TextDelta {
            index: ContentIndex::new(0),
            delta: text.to_owned(),
        }),
        Ok(AssistantEvent::TextEnd {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::Done {
            finish_reason: finish,
        }),
    ]
}

fn capabilities_supporting_json() -> CapabilitySet {
    CapabilitySet {
        response_format_json_object: CapabilityStatus::Supported,
        response_format_json_schema: CapabilityStatus::Supported,
        ..CapabilitySet::default()
    }
}

#[test]
fn token_count_unknown_differs_from_known_zero() {
    assert_ne!(TokenCount::Unknown, TokenCount::Known(0));
    assert_eq!(TokenCount::Known(0).known(), Some(0));
    assert!(TokenCount::Unknown.known().is_none());
}

#[test]
fn usage_merge_fills_unknown_is_idempotent_and_rejects_conflicting_known() {
    let partial = UsageDetails::new(
        TokenCount::Known(10),
        TokenCount::Unknown,
        TokenCount::Unknown,
        TokenCount::Unknown,
        TokenCount::Unknown,
        TokenCount::Unknown,
    );
    let complete = UsageDetails::new(
        TokenCount::Known(10),
        TokenCount::Known(4),
        TokenCount::Known(14),
        TokenCount::Known(2),
        TokenCount::Known(0),
        TokenCount::Known(1),
    );

    let (merged, outcome) = merge_usage_details(None, partial).unwrap();
    assert!(matches!(outcome, UsageMergeOutcome::EmitDetailed { .. }));

    let (merged, outcome) = merge_usage_details(Some(merged), complete).unwrap();
    assert!(matches!(outcome, UsageMergeOutcome::EmitP1 { .. }));
    assert_eq!(merged.reasoning_tokens(), TokenCount::Known(1));

    let (_, outcome) = merge_usage_details(Some(merged), complete).unwrap();
    assert_eq!(outcome, UsageMergeOutcome::Unchanged);

    let conflicting = UsageDetails::new(
        TokenCount::Known(11),
        TokenCount::Known(4),
        TokenCount::Known(15),
        TokenCount::Unknown,
        TokenCount::Unknown,
        TokenCount::Unknown,
    );
    let error = merge_usage_details(Some(merged), conflicting).unwrap_err();
    assert_eq!(error.reason(), CostFailure::InconsistentUsage);
}

#[test]
fn estimate_cost_missing_price_is_unknown_never_zero() {
    let usage = UsageDetails::new(
        TokenCount::Known(1_000_000),
        TokenCount::Known(500_000),
        TokenCount::Known(1_500_000),
        TokenCount::Known(200_000),
        TokenCount::Known(50_000),
        TokenCount::Known(100_000),
    );
    let estimate = estimate_cost(&usage, None).unwrap();
    assert!(estimate.currency().is_none());
    assert_eq!(estimate.input(), MoneyAmount::Unknown);
    assert_eq!(estimate.output(), MoneyAmount::Unknown);
    assert_eq!(estimate.total(), MoneyAmount::Unknown);
    assert!(estimate.price_version().is_none());
}

#[test]
fn estimate_cost_uses_price_profile_and_half_up_micros() {
    let usage = UsageDetails::new(
        TokenCount::Known(1_000_000),
        TokenCount::Known(500_000),
        TokenCount::Known(1_500_000),
        TokenCount::Known(200_000),
        TokenCount::Known(50_000),
        TokenCount::Known(100_000),
    );
    let price = PriceProfile::new(
        "2026-07-19",
        "fixture-price-book",
        CurrencyCode::new("USD").unwrap(),
        1_000_000, // $1 / M input
        2_000_000, // $2 / M output
        100_000,   // $0.10 / M cached input
        300_000,   // $0.30 / M cache write
    )
    .unwrap();
    let estimate = estimate_cost(&usage, Some(&price)).unwrap();
    // uncached input = 1_000_000 - 200_000 - 50_000 = 750_000 -> 750_000 micros
    assert_eq!(estimate.input(), MoneyAmount::Micros(750_000));
    assert_eq!(estimate.output(), MoneyAmount::Micros(1_000_000));
    assert_eq!(estimate.cached_input(), MoneyAmount::Micros(20_000));
    assert_eq!(estimate.cache_write(), MoneyAmount::Micros(15_000));
    assert_eq!(estimate.total(), MoneyAmount::Micros(1_785_000));
    assert_eq!(estimate.currency().unwrap().as_str(), "USD");
    assert_eq!(estimate.price_version(), Some("2026-07-19"));
    assert_eq!(estimate.price_source(), Some("fixture-price-book"));
}

#[tokio::test]
async fn collector_accepts_detailed_usage_and_preserves_p1_usage() {
    let events = stream::iter(vec![
        Ok(AssistantEvent::Start {
            local_request_id: LocalRequestId::new("local-p2").unwrap(),
            provider_request_id: None,
            generation_id: None,
        }),
        Ok(AssistantEvent::TextStart {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::TextDelta {
            index: ContentIndex::new(0),
            delta: "ok".to_owned(),
        }),
        Ok(AssistantEvent::TextEnd {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::Usage(Usage::new(2, 1, 3).unwrap())),
        Ok(AssistantEvent::DetailedUsage(UsageDetails::new(
            TokenCount::Known(2),
            TokenCount::Known(1),
            TokenCount::Known(3),
            TokenCount::Unknown,
            TokenCount::Known(0),
            TokenCount::Known(0),
        ))),
        Ok(AssistantEvent::Done {
            finish_reason: FinishReason::Stop,
        }),
    ]);
    let message = collect_assistant_message(events).await.unwrap();
    assert_eq!(message.usage().unwrap().total_tokens(), 3);
    assert_eq!(
        message.usage_details().unwrap().reasoning_tokens(),
        TokenCount::Known(0)
    );
    assert!(message.structured_output().is_none());
}

#[tokio::test]
async fn structured_json_object_validates_only_after_successful_stop() {
    let schema_events = text_events(r#"{"answer":42}"#, FinishReason::Stop);
    let message = collect_assistant_message_for_format(
        stream::iter(schema_events),
        &ResponseFormat::JsonObject,
    )
    .await
    .unwrap();
    assert_eq!(message.structured_output(), Some(&json!({"answer": 42})));

    let array_events = text_events("[1,2]", FinishReason::Stop);
    let error = collect_assistant_message_for_format(
        stream::iter(array_events),
        &ResponseFormat::JsonObject,
    )
    .await
    .unwrap_err();
    match error {
        LlmError::StructuredOutput(inner) => {
            assert_eq!(inner.reason(), StructuredOutputFailure::SchemaViolation);
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let truncated = text_events(r#"{"answer":"#, FinishReason::Length);
    let error =
        collect_assistant_message_for_format(stream::iter(truncated), &ResponseFormat::JsonObject)
            .await
            .unwrap_err();
    match error {
        LlmError::StructuredOutput(inner) => {
            assert_eq!(inner.reason(), StructuredOutputFailure::Truncated);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn structured_json_schema_validates_and_skips_tool_calls() {
    let schema = StructuredSchema::new(
        "answer_object",
        None,
        ToolSchema::new(json!({
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
    let format = ResponseFormat::JsonSchema(schema);

    let ok = collect_assistant_message_for_format(
        stream::iter(text_events(r#"{"answer":7}"#, FinishReason::Stop)),
        &format,
    )
    .await
    .unwrap();
    assert_eq!(ok.structured_output(), Some(&json!({"answer": 7})));

    let bad = collect_assistant_message_for_format(
        stream::iter(text_events(r#"{"answer":"nope"}"#, FinishReason::Stop)),
        &format,
    )
    .await
    .unwrap_err();
    assert!(matches!(bad, LlmError::StructuredOutput(_)));

    let tool_call = ToolCallLike::events();
    let tool_message = collect_assistant_message_for_format(stream::iter(tool_call), &format)
        .await
        .unwrap();
    assert!(tool_message.structured_output().is_none());
}

struct ToolCallLike;

impl ToolCallLike {
    fn events() -> Vec<Result<AssistantEvent, LlmError>> {
        use philo::{ToolArguments, ToolCall, ToolCallId, ToolName, WireToolIndex};
        let call = ToolCall::new(
            ToolCallId::new("call_1").unwrap(),
            ToolName::new("lookup").unwrap(),
            ToolArguments::from_raw_json(r#"{"q":"x"}"#).unwrap(),
        );
        vec![
            Ok(AssistantEvent::Start {
                local_request_id: LocalRequestId::new("local-p2").unwrap(),
                provider_request_id: None,
                generation_id: None,
            }),
            Ok(AssistantEvent::ToolCallStart {
                index: ContentIndex::new(0),
                wire_index: WireToolIndex::new(0),
                id: Some(call.id().clone()),
            }),
            Ok(AssistantEvent::ToolCallDelta {
                index: ContentIndex::new(0),
                wire_index: WireToolIndex::new(0),
                name_delta: Some("lookup".to_owned()),
                arguments_delta: Some(r#"{"q":"x"}"#.to_owned()),
            }),
            Ok(AssistantEvent::ToolCallEnd {
                index: ContentIndex::new(0),
                call,
            }),
            Ok(AssistantEvent::Done {
                finish_reason: FinishReason::ToolCalls,
            }),
        ]
    }
}

#[test]
fn response_format_capability_gates_unknown_and_unsupported() {
    let model = ModelRef::new("official-openai", "gpt-test").unwrap();
    let request = GenerateRequest::new(model, vec![Message::user("hi")])
        .with_options(GenerationOptions::new().with_response_format(ResponseFormat::JsonObject));

    let mut caps = capabilities_supporting_json();
    caps.response_format_json_object = CapabilityStatus::Unknown;
    assert!(matches!(
        request.validate(&caps),
        Err(LlmError::Capability(_))
    ));

    caps.response_format_json_object = CapabilityStatus::Unsupported;
    assert!(matches!(
        request.validate(&caps),
        Err(LlmError::Capability(_))
    ));

    caps.response_format_json_object = CapabilityStatus::Supported;
    assert!(request.validate(&caps).is_ok());
}

#[test]
fn official_dialect_policy_is_protocol_default_group() {
    let dialect = DialectPolicy::official_openai();
    assert_eq!(dialect.source, philo::PolicySource::ProtocolDefault);
    assert_eq!(
        dialect.structured_output,
        philo::StructuredOutputWireFormat::OpenAiResponseFormat
    );
    assert_eq!(dialect.stream_usage, philo::StreamUsagePolicy::IncludeUsage);
    assert_eq!(
        dialect.thinking,
        philo::ThinkingWireFormat::OpenAiReasoningEffort
    );
}

#[test]
fn capability_set_default_keeps_structured_output_unknown() {
    let caps = CapabilitySet::default();
    assert_eq!(caps.response_format_json_object, CapabilityStatus::Unknown);
    assert_eq!(caps.response_format_json_schema, CapabilityStatus::Unknown);
    assert!(matches!(
        caps.reasoning_efforts,
        ReasoningEffortSupport::Unknown
    ));
}

#[test]
fn model_reasoning_effort_set_is_exact_only() {
    let mut efforts = BTreeSet::new();
    efforts.insert(ReasoningEffort::Low);
    let caps = CapabilitySet {
        reasoning_efforts: ReasoningEffortSupport::Supported(efforts),
        ..CapabilitySet::default()
    };
    let model = ModelRef::new("official-openai", "gpt-test").unwrap();
    let request = GenerateRequest::new(model, vec![Message::user("hi")]).with_options(
        GenerationOptions::new()
            .with_reasoning(philo::ThinkingRequest::Effort(ReasoningEffort::High)),
    );
    assert!(matches!(
        request.validate(&caps),
        Err(LlmError::Capability(_))
    ));
}

#[tokio::test]
async fn intermediate_partial_json_is_not_validated_as_events() {
    // The collector still accepts incomplete intermediate text deltas. Only the
    // final Stop boundary is validated against the requested format.
    let events = stream::iter(vec![
        Ok(AssistantEvent::Start {
            local_request_id: LocalRequestId::new("local-p2").unwrap(),
            provider_request_id: None,
            generation_id: None,
        }),
        Ok(AssistantEvent::TextStart {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::TextDelta {
            index: ContentIndex::new(0),
            delta: r#"{"ans"#.to_owned(),
        }),
        Ok(AssistantEvent::TextDelta {
            index: ContentIndex::new(0),
            delta: r#"wer":1}"#.to_owned(),
        }),
        Ok(AssistantEvent::TextEnd {
            index: ContentIndex::new(0),
        }),
        Ok(AssistantEvent::Done {
            finish_reason: FinishReason::Stop,
        }),
    ]);
    let message = collect_assistant_message_for_format(events, &ResponseFormat::JsonObject)
        .await
        .unwrap();
    assert_eq!(message.text(), r#"{"answer":1}"#);
    assert!(matches!(message.content()[0], ContentPart::Text { .. }));
    assert_eq!(message.structured_output(), Some(&json!({"answer": 1})));
}
