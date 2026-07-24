//! `philo` is a secure, streaming-first Rust SDK for LLM applications.
//!
//! The foundation implements the frozen `philo/openai-chat-p1` contract and the
//! provider-independent domain follows `philo/openai-chat-p2`. The crate
//! exposes provider-independent domain types, a validated official provider runtime,
//! and [`LlmClient`] streaming/completion entry points. The request adapter and
//! `OpenAI` Chat wire/state types remain private; callers do not need reqwest, JSON,
//! or SSE implementation details.
//!
//! The protocol adapter supports official `OpenAI` Chat Completions text, function
//! tools, image inputs, structured output, usage/cost helpers, and reasoning-effort
//! request options. Phase-two still does not execute tools and does not claim
//! third-party thinking dialects.
//!
//! # Stability
//!
//! The public API is experimental during the `0.x` series. The phase-one behavior
//! contract is frozen, but Rust type layouts may change with release notes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// The public SDK, Cargo package, and library crate name.
pub const SDK_NAME: &str = "philo";

/// The version of this crate build.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The identifier of the frozen phase-one behavior contract.
pub const PHASE_ONE_CONTRACT_ID: &str = "philo/openai-chat-p1";

/// The version of the frozen phase-one behavior contract.
pub const PHASE_ONE_CONTRACT_VERSION: &str = "1.0.0";

/// The identifier of the frozen phase-two behavior contract.
pub const PHASE_TWO_CONTRACT_ID: &str = "philo/openai-chat-p2";

/// The version of the frozen phase-two behavior contract.
pub const PHASE_TWO_CONTRACT_VERSION: &str = "1.1.0";

/// The identifier of the versioned provider configuration schema.
pub const PROVIDER_CONFIG_SCHEMA_ID: &str = "philo/provider-config";

/// The current version of the provider configuration schema.
pub const PROVIDER_CONFIG_SCHEMA_VERSION: &str = "1.0";

pub mod client;
pub mod domain;
pub mod error;
mod execution;
pub mod observability;
mod protocol;
pub mod provider;
pub mod transport;

pub use client::{AssistantStream, LlmClient, RequestControl};
pub use domain::{
    AssistantEvent, AssistantMessage, CapabilitySet, CapabilityStatus, ContentIndex, ContentPart,
    CostEstimate, CurrencyCode, DiagnosticCode, DialectPolicy, FinishReason, GenerateRequest,
    GenerationId, GenerationOptions, HistoryCapabilities, HistoryPolicy, IdMapping, ImageContent,
    ImageDetail, ImageMime, ImageSource, ImageWireFormat, LlmRequest, LocalRequestId, Message,
    MessageRole, MissingToolResultPolicy, ModelId, ModelRef, MoneyAmount, NormalizationDiagnostic,
    NormalizedContext, OpaqueReasoning, ParallelToolCalls, PolicySource, PriceProfile, ProtocolId,
    ProviderId, ProviderRequestId, ReasoningEffort, ReasoningEffortSupport, RefusalContent,
    RequestMetadata, RequestTimeout, ResourceLimits, ResourceLimitsBuilder, ResponseFormat,
    SchemaLimits, SourceIdentity, StreamUsagePolicy, StructuredOutputWireFormat, StructuredSchema,
    ThinkingContent, ThinkingReplayPolicy, ThinkingRequest, ThinkingWireFormat, TokenCount,
    ToolArguments, ToolCall, ToolCallId, ToolCallIdPolicy, ToolChoice, ToolChoiceWireFormat,
    ToolDefinition, ToolLimits, ToolName, ToolResultMessage, ToolResultNamePolicy, ToolSchema,
    TraceId, UnsupportedContentPolicy, Usage, UsageDetails, UsageMergeOutcome, ValidatedToolCall,
    WireToolIndex, apply_thinking_replay_policy, collect_assistant_message,
    collect_assistant_message_for_format, drop_opaque_reasoning, estimate_cost,
    merge_usage_details, normalize_history, validate_tool_call, validate_tool_options,
};
pub use error::{
    AuthFailureKind, AuthenticationError, BodySummary, CapabilityError, CostError, CostFailure,
    CredentialError, CredentialFailure, ErrorStage, HeaderPolicyError, HeaderPolicyFailure,
    HistoryError, HistoryFailure, HttpStatusError, LlmError, ProtocolError, ProviderConfigError,
    ProviderConfigFailure, ProviderRegistryError, ProviderRegistryFailure, RetriableHint,
    SchemaError, SchemaFailure, StructuredOutputError, StructuredOutputFailure, TimeoutError,
    ToolValidationError, ToolValidationFailure, TransportError, TruncatedStreamError,
    UnknownFinishReason, ValidationError, ValidationReason,
};
pub use observability::{
    LifecycleErrorCategory, LifecycleEvent, LifecycleEventKind, LifecycleIdentity,
    LifecycleObserver,
};
pub use provider::{
    ApiKey, ApiKeyHeaderAuth, AuthContext, AuthDiagnostics, AuthProvider, AuthSchemeKind,
    BearerAuth, BearerCredential, CatalogCapabilities, CatalogDefaults, CatalogSource,
    CatalogSourceId, ClientIdentity, ClientIdentityConfig, ClientIdentityFragment,
    CompatDiagnostic, CompatField, CompatPatch, CompatProfile, ConfigSchemaVersion, ConfigSource,
    ConfigSourceId, ConfigSourceKind, ConfigSourceLocation, ConfigValue, ConstraintStrength,
    CredentialAudience, CredentialAudienceSpec, CredentialFuture, CredentialIdentity,
    CredentialSourceKind, DataRetention, DeepSeekProfile, DeploymentId, DetectionConfidence,
    DetectionExplanation, DetectionSuggestion, DetectionUnknownReason, DomainModelId, DynamicAuth,
    DynamicCredential, DynamicCredentialCache, DynamicCredentialContext, DynamicCredentialScheme,
    DynamicCredentialSource, DynamicHeaderContext, DynamicHeaderFuture, DynamicHeaderPolicy,
    DynamicHeaderSource, DynamicResponseFormat, EffectiveSupportStatus, EndpointConfig,
    EndpointDetection, EndpointDetectionPolicy, EndpointDetector, EndpointDiagnostics,
    EndpointNetworkPolicy, EndpointPathVariable, EndpointQuery, EndpointQueryAction,
    EndpointQueryDiagnostic, EndpointQuerySource, EndpointResolutionDiagnostics, EndpointSpec,
    EndpointTemplate, EndpointValues, EnvironmentSecretResolver, EvidenceVerification,
    FallbackDimension, FieldProvenance, FieldState, FinishReasonCompat, HeaderDiagnostic,
    HeaderLayer, HeaderOperation, HeaderPipeline, HeaderPolicy, HeaderSource, HeaderTraceEntry,
    HistoryCompat, InlineErrorCompat, ListMerge, MapMerge, MaxOutputTokensWireFormat,
    ModelBodyWireFormat, ModelCapabilityProfile, ModelCatalog, ModelEntry, ModelKey, ModelLimits,
    MultiHeaderAuth, NamedConfigValue, NamedListMerge, NoAuth, NormalizedEndpointFacts,
    OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE, OfficialOpenAiFactory, OfficialOpenAiProfile,
    OpenRouterAttribution, OpenRouterProfile, OpenRouterRoutingContract, OpenRouterRoutingPatch,
    Origin, ProductId, ProtocolDialect, ProviderCapabilities, ProviderConfigDocument,
    ProviderConfigField, ProviderConfigLayer, ProviderConfigSnapshot, ProviderDiagnostics,
    ProviderModelId, ProviderProfile, ProviderRegistration, ProviderRegistrationMetadata,
    ProviderRegistry, ProviderRequestOptions, ProviderRuntime, ProviderRuntimeFactory,
    ProviderSelection, ProviderSelectionInput, ProviderSelectionSource, ProviderSelector,
    ProviderTransportOptions, QueryMergeRule, RedirectPolicy, RequestCompat, ResolvedEndpoint,
    ResolvedHeaders, ResolvedModelMapping, ResolvedProviderRouting, ResponseCompat,
    RoutingFallback, RoutingField, RoutingRegion, RoutingSort, SecretReference, SecretResolver,
    SensitiveHeaderValue, SupportDiagnostics, SupportStatus, TenantId, ToolArgumentsCompat,
    TraceDecision, TraceOperation, UpstreamId, UsageCompat, WireModelValue, ZaiCodingProfile,
    ZaiStandardProfile, resolve_compat,
};
pub use transport::{
    ByteStream, CancellationToken, HttpRequest, HttpResponse, LimitedBody, RequestLifecycle,
    ReqwestTransport, SseConfig, SseDecoder, SseError, SseEvent, SseLimit, Transport,
    TransportContext, TransportFuture, read_body_limited,
};

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::{error::Error as _, io};

    use futures_util::stream;
    use http::{HeaderName, HeaderValue};

    use crate::domain::event::{
        AssistantEvent, FinishReason, LocalRequestId, Usage, collect_assistant_message,
    };
    use crate::domain::request::{
        CapabilitySet, CapabilityStatus, GenerateRequest, GenerationOptions,
    };
    use crate::domain::{
        ContentIndex, ContentPart, Message, MessageRole, ModelId, ModelRef, ProtocolId, ProviderId,
    };
    use crate::error::{
        AuthFailureKind, AuthenticationError, BodySummary, ErrorStage, HttpStatusError, LlmError,
        RetriableHint, TimeoutError, TransportError, ValidationReason,
    };

    use super::{
        PHASE_ONE_CONTRACT_ID, PHASE_ONE_CONTRACT_VERSION, PHASE_TWO_CONTRACT_ID,
        PHASE_TWO_CONTRACT_VERSION, SDK_NAME, SDK_VERSION,
    };

    #[test]
    fn published_metadata_matches_frozen_decisions() {
        assert_eq!(SDK_NAME, "philo");
        assert_eq!(SDK_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(PHASE_ONE_CONTRACT_ID, "philo/openai-chat-p1");
        assert_eq!(PHASE_ONE_CONTRACT_VERSION, "1.0.0");
        assert_eq!(PHASE_TWO_CONTRACT_ID, "philo/openai-chat-p2");
        assert_eq!(PHASE_TWO_CONTRACT_VERSION, "1.1.0");
    }

    #[test]
    fn identifiers_reject_empty_and_boundary_whitespace_but_preserve_internal_spaces() {
        assert_eq!(ProviderId::new("open ai").unwrap().as_str(), "open ai");
        assert_eq!(
            ProtocolId::new("openai-chat").unwrap().as_str(),
            "openai-chat"
        );
        assert_eq!(ModelId::new("model v1").unwrap().as_str(), "model v1");
        assert!(matches!(
            ProviderId::new("").unwrap_err().reason(),
            ValidationReason::Empty
        ));
        assert!(matches!(
            ProtocolId::new(" x").unwrap_err().reason(),
            ValidationReason::BoundaryWhitespace
        ));
        assert!(matches!(
            ModelId::new("x ").unwrap_err().reason(),
            ValidationReason::BoundaryWhitespace
        ));
    }

    #[test]
    fn messages_preserve_text_and_roles() {
        let text = "  保留换行\n";
        let message = Message::new(MessageRole::Developer, vec![ContentPart::text(text)]);
        assert_eq!(message.role(), MessageRole::Developer);
        assert_eq!(message.content()[0].as_text(), text);
        assert_eq!(Message::system("s").role(), MessageRole::System);
        assert_eq!(Message::user("u").role(), MessageRole::User);
        assert_eq!(Message::assistant("a").role(), MessageRole::Assistant);
    }

    fn valid_request() -> GenerateRequest {
        GenerateRequest::new(
            ModelRef::new("openai", "gpt-test").unwrap(),
            vec![Message::user("hello")],
        )
    }

    #[test]
    fn request_validation_is_fail_closed_and_does_not_expose_values() {
        let invalid =
            valid_request().with_options(GenerationOptions::new().with_temperature(f64::NAN));
        let error = invalid.validate(&CapabilitySet::default()).unwrap_err();
        assert!(
            matches!(error, LlmError::Validation(ref error) if error.reason() == ValidationReason::NonFinite)
        );
        assert!(!error.to_string().contains("NaN"));
        let unsupported =
            valid_request().with_options(GenerationOptions::new().with_max_output_tokens(10));
        let capabilities = CapabilitySet {
            max_output_tokens: CapabilityStatus::Unknown,
            ..CapabilitySet::default()
        };
        assert!(matches!(
            unsupported.validate(&capabilities),
            Err(LlmError::Capability(_))
        ));
        let protected = valid_request().with_options(GenerationOptions::new().with_header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("secret"),
        ));
        let error = protected.validate(&CapabilitySet::default()).unwrap_err();
        assert!(
            matches!(error, LlmError::Validation(ref error) if error.reason() == ValidationReason::ProtectedHeader)
        );
        assert!(!error.to_string().contains("secret"));
        assert!(
            GenerationOptions::new()
                .with_timeout(Duration::ZERO)
                .is_err()
        );
    }

    #[test]
    fn invalid_request_never_reaches_transport_spy() {
        let calls = std::cell::Cell::new(0_u32);
        let request = GenerateRequest::new(
            ModelRef::new("openai", "gpt-test").unwrap(),
            vec![Message::system("no user")],
        );
        let result = request.validate(&CapabilitySet::default()).map(|()| {
            calls.set(calls.get() + 1);
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn request_validation_covers_message_and_numeric_decision_table() {
        let model = || ModelRef::new("openai", "gpt-test").unwrap();
        let cases = [
            GenerateRequest::new(model(), vec![]),
            GenerateRequest::new(model(), vec![Message::assistant("answer only")]),
            GenerateRequest::new(model(), vec![Message::user(" \t\n")]),
            valid_request().with_options(GenerationOptions::new().with_temperature(-0.1)),
            valid_request().with_options(GenerationOptions::new().with_temperature(2.1)),
            valid_request().with_options(GenerationOptions::new().with_max_output_tokens(0)),
        ];
        for request in cases {
            assert!(request.validate(&CapabilitySet::default()).is_err());
        }
        assert!(valid_request().validate(&CapabilitySet::default()).is_ok());
        assert!(
            GenerateRequest::new(
                model(),
                vec![Message::new(
                    MessageRole::User,
                    vec![ContentPart::text("a"), ContentPart::text("b")],
                )],
            )
            .validate(&CapabilitySet::default())
            .is_ok()
        );
    }

    fn events() -> Vec<Result<AssistantEvent, LlmError>> {
        vec![
            Ok(AssistantEvent::start(
                LocalRequestId::new("local-1").unwrap(),
            )),
            Ok(AssistantEvent::TextStart {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::TextDelta {
                index: ContentIndex::new(0),
                delta: "你".into(),
            }),
            Ok(AssistantEvent::TextDelta {
                index: ContentIndex::new(0),
                delta: "好".into(),
            }),
            Ok(AssistantEvent::TextEnd {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::Usage(Usage::new(1, 2, 3).unwrap())),
            Ok(AssistantEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ]
    }

    #[tokio::test]
    async fn collector_preserves_unicode_and_usage() {
        let message = collect_assistant_message(stream::iter(events()))
            .await
            .unwrap();
        assert_eq!(message.text(), "你好");
        assert_eq!(message.usage().unwrap().total_tokens(), 3);
        assert_eq!(message.finish_reason(), &FinishReason::Stop);
    }

    #[tokio::test]
    async fn collector_rejects_duplicate_done_and_truncation() {
        let mut duplicate = events();
        duplicate.push(Ok(AssistantEvent::Done {
            finish_reason: FinishReason::Stop,
        }));
        assert!(matches!(
            collect_assistant_message(stream::iter(duplicate)).await,
            Err(LlmError::Protocol(_))
        ));
        let mut truncated = events();
        truncated.pop();
        assert!(matches!(
            collect_assistant_message(stream::iter(truncated)).await,
            Err(LlmError::TruncatedStream(_))
        ));
        let mut failed = events();
        failed.insert(4, Err(LlmError::Cancelled));
        assert!(matches!(
            collect_assistant_message(stream::iter(failed)).await,
            Err(LlmError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn empty_text_completion_has_explicit_boundaries_and_unknown_usage() {
        let events = vec![
            Ok(AssistantEvent::TextStart {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::TextEnd {
                index: ContentIndex::new(0),
            }),
            Ok(AssistantEvent::Done {
                finish_reason: FinishReason::Length,
            }),
        ];
        let message = collect_assistant_message(stream::iter(events))
            .await
            .unwrap();
        assert_eq!(message.text(), "");
        assert_eq!(message.usage(), None);
        assert_eq!(message.finish_reason(), &FinishReason::Length);
    }

    #[test]
    fn bounded_body_is_safe_for_binary_and_long_input() {
        let body = BodySummary::from_bytes(&[0xff, b'a', b'b', b'c'], 2);
        assert!(body.as_str().contains("... [truncated]"));
        assert!(body.as_str().contains('\u{fffd}'));
    }

    #[test]
    fn error_taxonomy_is_typed_and_diagnostics_are_redacted() {
        let authentication = LlmError::from(AuthenticationError::new(
            AuthFailureKind::Permission,
            ErrorStage::Http,
            RetriableHint::No,
        ));
        assert!(
            matches!(authentication, LlmError::Authentication(ref error) if error.kind() == AuthFailureKind::Permission)
        );

        let transport = LlmError::from(TransportError::with_source(
            ErrorStage::Connect,
            RetriableHint::Maybe,
            io::Error::other("canary-secret-value"),
        ));
        assert!(transport.source().is_some());
        assert!(!format!("{transport:?}").contains("canary-secret-value"));
        assert!(!transport.to_string().contains("canary-secret-value"));

        let body = BodySummary::from_bytes(br#"{"api_key":"canary-secret-value"}"#, 1024);
        let http = LlmError::from(HttpStatusError::new(429, body, None, RetriableHint::Maybe));
        assert!(!format!("{http:?}").contains("canary-secret-value"));
        assert!(!http.to_string().contains("canary-secret-value"));

        assert!(matches!(
            LlmError::from(TimeoutError::new(ErrorStage::Timeout)),
            LlmError::Timeout(_)
        ));
        assert!(matches!(LlmError::Cancelled, LlmError::Cancelled));
    }
}
