//! Protocol response dispatch for one completed HTTP attempt.

use http::StatusCode;

use crate::error::{HttpStatusError, LlmError, RetriableHint};
use crate::execution::executor::{AttemptResponse, AttemptResponseBody};
use crate::protocol::openai_chat::{
    OpenAiChatStreamContext, decode_openai_chat_stream_with_policy,
};

use super::{EventStream, OpenAiChatResponsePlan, ProtocolResponsePlan, ResponseMeta};

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
        }
    }
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
        *plan.compat.profile.response(),
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

    use crate::domain::{DialectPolicy, LocalRequestId, ModelRef, ResourceLimits, ResponseFormat};
    use crate::execution::executor::{AttemptResponse, AttemptResponseBody};
    use crate::protocol::{
        ExpectedContentType, HttpResponseRequirements, OpenAiChatResponsePlan,
        ProtocolResponsePlan, ResponseMeta, ResponsePlan,
    };
    use crate::provider::call_policy::{ResolvedCompat, ResponseLimits};
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
                    compat: ResolvedCompat {
                        dialect: DialectPolicy::official_openai(),
                        profile: crate::provider::CompatProfile::openai_chat_default(),
                    },
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
}
