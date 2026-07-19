//! Phase-two multimodal and thinking/reasoning contract tests.

use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::Bytes;
use philo::{
    CapabilitySet, CapabilityStatus, ContentPart, DiagnosticCode, GenerateRequest,
    GenerationOptions, HistoryCapabilities, HistoryFailure, HistoryPolicy, ImageContent,
    ImageDetail, ImageMime, ImageSource, Message, MessageRole, ModelId, OpaqueReasoning,
    ProtocolId, ProviderId, ReasoningEffort, ReasoningEffortSupport, ResourceLimits,
    SourceIdentity, ThinkingContent, ThinkingReplayPolicy, ThinkingRequest, TokenCount,
    ToolResultMessage, UsageDetails, apply_thinking_replay_policy, normalize_history,
};

fn png_bytes() -> Bytes {
    Bytes::from_static(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 1, 2, 3])
}

fn jpeg_bytes() -> Bytes {
    Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9])
}

fn vision_capabilities() -> CapabilitySet {
    CapabilitySet {
        vision_input: CapabilityStatus::Supported,
        image_detail_original: CapabilityStatus::Supported,
        ..CapabilitySet::default()
    }
}

fn reasoning_capabilities() -> CapabilitySet {
    CapabilitySet {
        reasoning_efforts: ReasoningEffortSupport::Supported(BTreeSet::from([
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::High,
        ])),
        ..CapabilitySet::default()
    }
}

#[test]
fn image_constructors_validate_scheme_mime_magic_and_limits() {
    assert!(ImageContent::parse_url("http://example.com/a.png", ImageDetail::Auto).is_err());
    assert!(
        ImageContent::parse_url(
            &format!("https://example.com/{}", "a".repeat(9000)),
            ImageDetail::Auto
        )
        .is_err()
    );
    let ok = ImageContent::parse_url(
        "https://example.com/a.png?token=query-canary",
        ImageDetail::Auto,
    )
    .unwrap();
    assert!(!format!("{ok:?}").contains("query-canary"));

    assert!(ImageContent::from_inline(ImageMime::Png, Bytes::new(), ImageDetail::Auto).is_err());
    assert!(ImageContent::from_inline(ImageMime::Png, jpeg_bytes(), ImageDetail::Auto).is_err());
    assert!(ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Low).is_ok());

    let data_url = format!(
        "data:image/png;base64,{}",
        BASE64_STANDARD.encode(png_bytes())
    );
    assert!(ImageContent::from_data_url(&data_url, ImageDetail::Auto).is_ok());
    assert!(ImageContent::from_data_url("data: image/png;base64,AAA", ImageDetail::Auto).is_err());
    assert!(
        ImageContent::from_data_url("data:image/png;base64,not-base64!!!", ImageDetail::Auto)
            .is_err()
    );
}

#[test]
fn request_validation_enforces_image_and_reasoning_capabilities() {
    let image_message = Message::new(
        MessageRole::User,
        vec![
            ContentPart::text("look"),
            ContentPart::Image(
                ImageContent::parse_url("https://example.com/a.png", ImageDetail::Original)
                    .unwrap(),
            ),
        ],
    );
    let request = GenerateRequest::new(
        philo::ModelRef::new("openai", "gpt-test").unwrap(),
        vec![image_message],
    );

    let mut unsupported = vision_capabilities();
    unsupported.vision_input = CapabilityStatus::Unknown;
    assert!(request.validate(&unsupported).is_err());

    let mut original = vision_capabilities();
    original.image_detail_original = CapabilityStatus::Unknown;
    assert!(request.validate(&original).is_err());
    assert!(request.validate(&vision_capabilities()).is_ok());

    let reasoning = GenerateRequest::new(
        philo::ModelRef::new("openai", "gpt-test").unwrap(),
        vec![Message::user("hello")],
    )
    .with_options(
        GenerationOptions::new().with_reasoning(ThinkingRequest::Effort(ReasoningEffort::High)),
    );
    assert!(reasoning.validate(&CapabilitySet::default()).is_err());
    assert!(reasoning.validate(&reasoning_capabilities()).is_ok());
    assert_eq!(
        GenerationOptions::default().reasoning(),
        ThinkingRequest::ProviderDefault
    );
}

#[test]
fn tool_result_images_fail_closed_and_history_rejects_unsupported_images() {
    let call_id = philo::ToolCallId::new("call_1").unwrap();
    let name = philo::ToolName::new("tool").unwrap();
    let image = ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Auto).unwrap();
    let err = ToolResultMessage::new(
        call_id,
        name,
        vec![ContentPart::Image(image.clone())],
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(err.reason(), HistoryFailure::UnsupportedContent);

    let history = normalize_history(
        &[Message::new(
            MessageRole::User,
            vec![ContentPart::text("look"), ContentPart::Image(image)],
        )],
        &HistoryCapabilities::official_openai_defaults(),
        &philo::DialectPolicy::official_openai(),
        &HistoryPolicy::official_openai(),
    )
    .unwrap_err();
    assert_eq!(history.reason(), HistoryFailure::UnsupportedContent);
}

#[test]
fn thinking_replay_policy_helper_drops_or_retains_opaque_data() {
    let source = SourceIdentity::new(
        ProviderId::new("provider").unwrap(),
        ModelId::new("model-a").unwrap(),
        ProtocolId::new("protocol").unwrap(),
    );
    let other = SourceIdentity::new(
        ProviderId::new("provider").unwrap(),
        ModelId::new("model-b").unwrap(),
        ProtocolId::new("protocol").unwrap(),
    );
    let thinking = ThinkingContent::new("visible-canary").with_opaque(OpaqueReasoning::new(
        Bytes::from_static(b"opaque-canary"),
        source.clone(),
        false,
    ));

    let (dropped, diagnostics) =
        apply_thinking_replay_policy(&thinking, ThinkingReplayPolicy::DropAll, Some(&source));
    assert!(dropped.is_none());
    assert_eq!(diagnostics[0].code(), DiagnosticCode::DroppedThinkingOpaque);

    let (same, diagnostics) = apply_thinking_replay_policy(
        &thinking,
        ThinkingReplayPolicy::SameSourceOnly,
        Some(&source),
    );
    assert!(same.unwrap().opaque().is_some());
    assert!(diagnostics.is_empty());

    let (different, diagnostics) = apply_thinking_replay_policy(
        &thinking,
        ThinkingReplayPolicy::SameSourceOnly,
        Some(&other),
    );
    assert!(different.unwrap().opaque().is_none());
    assert_eq!(diagnostics[0].code(), DiagnosticCode::DroppedThinkingOpaque);

    let debug = format!("{thinking:?}");
    assert!(!debug.contains("visible-canary"));
    assert!(!debug.contains("opaque-canary"));
}

#[test]
fn usage_details_preserve_unknown_versus_known_zero_and_reasoning_tokens() {
    let details = UsageDetails::new(
        TokenCount::Known(10),
        TokenCount::Known(4),
        TokenCount::Known(14),
        TokenCount::Unknown,
        TokenCount::Known(0),
        TokenCount::Known(2),
    );
    assert_eq!(details.input_tokens(), TokenCount::Known(10));
    assert_eq!(details.reasoning_tokens(), TokenCount::Known(2));
    assert_eq!(details.cache_write_tokens(), TokenCount::Known(0));
    assert_eq!(details.cached_input_tokens(), TokenCount::Unknown);
    assert!(details.has_any_known());
}

#[test]
fn image_source_enum_keeps_order_semantics_in_content_parts() {
    let parts = [
        ContentPart::text("compare"),
        ContentPart::Image(
            ImageContent::parse_url("https://example.com/a.png", ImageDetail::High).unwrap(),
        ),
        ContentPart::text("and"),
        ContentPart::Image(
            ImageContent::from_inline(ImageMime::Png, png_bytes(), ImageDetail::Low).unwrap(),
        ),
    ];
    assert!(matches!(parts[0], ContentPart::Text { .. }));
    assert!(matches!(parts[1], ContentPart::Image(_)));
    assert!(matches!(parts[2], ContentPart::Text { .. }));
    assert!(matches!(parts[3], ContentPart::Image(_)));
    if let ContentPart::Image(image) = &parts[1] {
        assert!(matches!(image.source(), ImageSource::Url(_)));
        assert_eq!(image.detail(), ImageDetail::High);
    }
    assert!(ResourceLimits::official().max_images >= 1);
}
