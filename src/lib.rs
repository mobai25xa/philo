//! `philo` is a secure, streaming-first Rust SDK for LLM applications. It exposes
//! provider-independent requests and events while keeping protocol wire types,
//! credentials, retries, and network policy behind typed boundaries.
//!
//! Official `OpenAI` Chat Completions is part of the Stable candidate. Anthropic
//! Messages, companion packages, provider presets, and protocol-specific thinking
//! remain Experimental until their stated release gates pass. Bounded raw body
//! extensions are explicit escape hatches: their safety contract is Stable, but
//! their Provider semantics are not portable.
//!
//! Unknown model capabilities fail closed. The SDK validates Tool calls but never
//! authorizes or executes them. See the repository README for the minimal call,
//! Provider selection, maintained examples, and security boundary.
//!
//! # Stability
//!
//! The public API remains pre-1.0 until the first protected Stable tag creates its
//! API baseline. Stable-candidate items follow the repository compatibility policy;
//! Experimental items may change in a Minor release with migration notes.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// The public SDK, Cargo package, and library crate name.
pub const SDK_NAME: &str = "philo";

/// The version of this crate build.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The identifier of the current `OpenAI` Chat behavior contract.
pub const OPENAI_CHAT_CONTRACT_ID: &str = "philo/openai-chat";

/// The version of the current `OpenAI` Chat behavior contract.
pub const OPENAI_CHAT_CONTRACT_VERSION: &str = "1.1.0";

/// The identifier of the production reliability contract.
pub const RELIABILITY_CONTRACT_ID: &str = "philo/reliability";

/// The version of the production reliability contract.
pub const RELIABILITY_CONTRACT_VERSION: &str = "1.0.0";

/// The identifier of the versioned provider configuration schema.
pub const PROVIDER_CONFIG_SCHEMA_ID: &str = "philo/provider-config";

/// The current version of the provider configuration schema.
pub const PROVIDER_CONFIG_SCHEMA_VERSION: &str = "1.1";

pub mod client;
pub mod domain;
pub mod error;
mod execution;
pub mod observability;
mod plan;
pub mod protected;
mod protocol;
pub mod protocol_options;
pub mod provider;
pub mod transport;

// The crate root exports only what a first request needs: construct a runtime,
// build a request, stream it, handle the error. Everything else stays public at
// its owning module path; see `docs/maintenance/public-api-inventory.md`.
pub use client::{AssistantStream, LlmClient, RequestControl};
pub use domain::{
    AssistantEvent, AssistantMessage, ContentPart, FinishReason, GenerateRequest,
    GenerationOptions, Message, MessageRole, ModelId, ModelRef, ProviderId, TokenCount, Usage,
    UsageDetails,
};
pub use error::LlmError;
pub use execution::reliability::{RetryPolicy, RetryWaitPolicy, TimeoutPolicy};
pub use protocol_options::ProtocolOptions;
pub use provider::{
    ProviderDefinition, ProviderDefinitionBuilder, ProviderDeploymentConfig, ProviderRuntime,
};
pub use transport::{ReqwestTransport, Transport};

#[cfg(test)]
extern crate self as philo;

#[cfg(test)]
#[path = "../tests/support/http_server.rs"]
mod test_http_server;

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
        OPENAI_CHAT_CONTRACT_ID, OPENAI_CHAT_CONTRACT_VERSION, RELIABILITY_CONTRACT_ID,
        RELIABILITY_CONTRACT_VERSION, SDK_NAME, SDK_VERSION,
    };

    #[test]
    fn published_metadata_matches_frozen_decisions() {
        assert_eq!(SDK_NAME, "philo");
        assert_eq!(SDK_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(OPENAI_CHAT_CONTRACT_ID, "philo/openai-chat");
        assert_eq!(OPENAI_CHAT_CONTRACT_VERSION, "1.1.0");
        assert_eq!(RELIABILITY_CONTRACT_ID, "philo/reliability");
        assert_eq!(RELIABILITY_CONTRACT_VERSION, "1.0.0");
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
