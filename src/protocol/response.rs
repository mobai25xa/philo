//! Protocol response dispatch for one completed HTTP attempt.

use http::StatusCode;

use crate::error::{HttpStatusError, LlmError, RetriableHint};
use crate::execution::executor::{AttemptResponse, AttemptResponseBody};
use crate::protocol::anthropic_messages::{
    AnthropicMessagesStreamContext, decode_anthropic_messages_stream, decode_http_error,
};
use crate::protocol::openai_chat::{
    OpenAiChatStreamContext, decode_openai_chat_stream_with_policy,
};

use super::{
    AnthropicMessagesResponsePlan, EventStream, OpenAiChatResponsePlan, ProtocolResponsePlan,
    ResponseMeta,
};

/// Opens the concrete protocol session selected by an owned response plan.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ResponseSession;

impl ResponseSession {
    pub(crate) fn open(response: AttemptResponse) -> Result<EventStream, LlmError> {
        match (response.plan.protocol, response.outcome) {
            (ProtocolResponsePlan::OpenAiChat(plan), AttemptResponseBody::Success(body)) => {
                Ok(open_openai_chat(plan, response.meta, body))
            }
            (ProtocolResponsePlan::OpenAiChat(_), AttemptResponseBody::HttpFailure(body)) => {
                Err(HttpStatusError::new(
                    response.meta.status.as_u16(),
                    body.summary(),
                    response.meta.provider_request_id,
                    status_retriable(response.meta.status),
                )
                .with_retry_after(response.meta.retry_after)
                .with_rate_limit(response.meta.rate_limit)
                .into())
            }
            (ProtocolResponsePlan::AnthropicMessages(plan), AttemptResponseBody::Success(body)) => {
                Ok(open_anthropic_messages(plan, response.meta, body))
            }
            (
                ProtocolResponsePlan::AnthropicMessages(_),
                AttemptResponseBody::HttpFailure(body),
            ) => {
                let details = decode_http_error(&body);
                Err(HttpStatusError::new(
                    response.meta.status.as_u16(),
                    details.summary,
                    response.meta.provider_request_id.or(details.request_id),
                    status_retriable(response.meta.status),
                )
                .with_provider_code(details.provider_code)
                .with_retry_after(response.meta.retry_after)
                .with_rate_limit(response.meta.rate_limit)
                .into())
            }
        }
    }
}

fn open_anthropic_messages(
    plan: AnthropicMessagesResponsePlan,
    meta: ResponseMeta,
    body: crate::transport::ByteStream,
) -> EventStream {
    let context = AnthropicMessagesStreamContext::new(
        meta.local_request_id,
        meta.provider_request_id,
        plan.source,
    );
    Box::pin(decode_anthropic_messages_stream(
        body,
        context,
        plan.response_format,
        plan.sse,
        plan.limits,
        plan.contract,
    ))
}

fn open_openai_chat(
    plan: OpenAiChatResponsePlan,
    meta: ResponseMeta,
    body: crate::transport::ByteStream,
) -> EventStream {
    let context =
        OpenAiChatStreamContext::new(meta.local_request_id, meta.provider_request_id, plan.model);
    Box::pin(decode_openai_chat_stream_with_policy(
        body,
        context,
        plan.response_format,
        plan.sse,
        plan.limits,
        *plan.contract.compat().response(),
    ))
}

fn status_retriable(status: StatusCode) -> RetriableHint {
    if status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        RetriableHint::Maybe
    } else {
        RetriableHint::No
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::{StreamExt as _, stream};
    use http::StatusCode;

    use crate::domain::{
        LocalRequestId, ModelId, ModelRef, ProtocolId, ProviderId, ResourceLimits, ResponseFormat,
        SourceIdentity,
    };
    use crate::execution::executor::{AttemptResponse, AttemptResponseBody};
    use crate::plan::ResponseLimits;
    use crate::protocol::{
        AnthropicMessagesResponsePlan, ExpectedContentType, HttpResponseRequirements,
        OpenAiChatResponsePlan, ProtocolResponsePlan, ResponseMeta, ResponsePlan,
    };
    use crate::transport::LimitedBody;
    use crate::transport::{ByteStream, SseConfig};

    use super::ResponseSession;

    fn response(input: &'static [u8], format: ResponseFormat) -> AttemptResponse {
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(input))]));
        AttemptResponse {
            plan: ResponsePlan {
                http: HttpResponseRequirements {
                    content_type: ExpectedContentType::EventStream,
                    max_error_body_bytes: 64,
                },
                protocol: ProtocolResponsePlan::OpenAiChat(OpenAiChatResponsePlan {
                    model: ModelRef::new("test-only", "gpt-test").unwrap(),
                    response_format: format,
                    contract: crate::provider::OpenAiChatContract::strict(),
                    limits: ResponseLimits::from(ResourceLimits::official()),
                    sse: SseConfig::default(),
                }),
            },
            meta: ResponseMeta {
                local_request_id: LocalRequestId::new("response-session").unwrap(),
                provider_request_id: None,
                status: StatusCode::OK,
                header_names: Vec::new(),
                retry_after: None,
                rate_limit: crate::provider::observe_rate_limit(
                    StatusCode::OK,
                    &http::HeaderMap::new(),
                    &crate::provider::RateLimitPolicy::standard_only(),
                    crate::provider::RateLimitValue::Unknown,
                    std::time::SystemTime::now(),
                ),
            },
            outcome: AttemptResponseBody::Success(body),
        }
    }

    fn anthropic_response(input: &'static [u8]) -> AttemptResponse {
        let body: ByteStream = Box::pin(stream::iter([Ok(Bytes::from_static(input))]));
        AttemptResponse {
            plan: ResponsePlan {
                http: HttpResponseRequirements {
                    content_type: ExpectedContentType::EventStream,
                    max_error_body_bytes: 64,
                },
                protocol: ProtocolResponsePlan::AnthropicMessages(AnthropicMessagesResponsePlan {
                    source: SourceIdentity::new(
                        ProviderId::new("test-only").unwrap(),
                        ModelId::new("claude-test").unwrap(),
                        ProtocolId::new("anthropic-messages").unwrap(),
                    ),
                    response_format: ResponseFormat::Text,
                    contract: crate::provider::AnthropicMessagesContract::strict_official(),
                    limits: ResponseLimits::from(ResourceLimits::official()),
                    sse: SseConfig::default(),
                }),
            },
            meta: ResponseMeta {
                local_request_id: LocalRequestId::new("anthropic-response-session").unwrap(),
                provider_request_id: None,
                status: StatusCode::OK,
                header_names: Vec::new(),
                retry_after: None,
                rate_limit: crate::provider::observe_rate_limit(
                    StatusCode::OK,
                    &http::HeaderMap::new(),
                    &crate::provider::RateLimitPolicy::standard_only(),
                    crate::provider::RateLimitValue::Unknown,
                    std::time::SystemTime::now(),
                ),
            },
            outcome: AttemptResponseBody::Success(body),
        }
    }

    #[tokio::test]
    async fn response_session_is_the_structured_success_boundary() {
        let input = b"data: {\"id\":\"structured\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"not-json\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let results = ResponseSession::open(response(input, ResponseFormat::JsonObject))
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            results.last(),
            Some(Err(crate::error::LlmError::StructuredOutput(_)))
        ));
        assert!(
            !results
                .iter()
                .any(|result| matches!(result, Ok(crate::domain::AssistantEvent::Done { .. })))
        );
    }

    #[tokio::test]
    async fn response_session_ignores_empty_usage_without_losing_done() {
        let input = include_bytes!("../../tests/fixtures/phase-2/repair/response/empty-usage.sse");
        let events = ResponseSession::open(response(input, ResponseFormat::Text))
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!events.iter().any(|event| matches!(
            event,
            crate::domain::AssistantEvent::Usage(_)
                | crate::domain::AssistantEvent::DetailedUsage(_)
        )));
        assert!(matches!(
            events.last(),
            Some(crate::domain::AssistantEvent::Done { .. })
        ));
    }

    #[tokio::test]
    async fn response_session_dispatches_anthropic_with_fresh_state_per_attempt() {
        let input =
            include_bytes!("../../tests/fixtures/phase-5/anthropic-messages/stream/text.sse");
        for _ in 0..2 {
            let events = ResponseSession::open(anthropic_response(input))
                .unwrap()
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(matches!(
                events.first(),
                Some(crate::domain::AssistantEvent::Start { .. })
            ));
            assert!(matches!(
                events.last(),
                Some(crate::domain::AssistantEvent::Done { .. })
            ));
        }
    }

    #[test]
    fn anthropic_http_error_retains_safe_code_and_body_request_id_only() {
        let mut response = anthropic_response(b"");
        response.meta.status = StatusCode::BAD_REQUEST;
        response.outcome = AttemptResponseBody::HttpFailure(LimitedBody::from_test_parts(
            Bytes::from_static(
                br#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt-output-canary"},"request_id":"req_body_test"}"#,
            ),
            false,
        ));
        let Err(error) = ResponseSession::open(response) else {
            panic!("HTTP failure must not open a stream");
        };
        let crate::error::LlmError::HttpStatus(error) = error else {
            panic!("expected HTTP status error");
        };
        assert_eq!(error.provider_code(), Some("invalid_request_error"));
        assert_eq!(error.request_id().unwrap().as_str(), "req_body_test");
        assert!(!format!("{error:?}").contains("prompt-output-canary"));
    }
}
