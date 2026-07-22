//! One-attempt endpoint, header, authentication, and HTTP execution.

use std::fmt;
use std::sync::Arc;

use http::{HeaderMap, header};
use tokio::time::Instant;

use crate::domain::{LocalRequestId, ProviderRequestId};
use crate::error::{LlmError, ProtocolError, ValidationError, ValidationReason};
use crate::observability::{
    LifecycleEvent, LifecycleEventKind, LifecycleIdentity, LifecycleObserver,
};
use crate::protocol::{
    ExpectedContentType, PreparedCall, ProtocolOperation, ResponseMeta, ResponsePlan,
};
use crate::provider::ProviderRuntime;
use crate::transport::{
    ByteStream, HttpRequest, LimitedBody, RequestLifecycle, Transport, TransportContext,
    lifecycle_preflight, read_body_limited,
};

/// Value-free observation context retained for one logical call.
#[derive(Clone)]
pub(crate) struct AttemptObservation {
    observer: Arc<dyn LifecycleObserver>,
    identity: Arc<LifecycleIdentity>,
    started: Instant,
}

impl AttemptObservation {
    pub(crate) fn new(
        observer: Arc<dyn LifecycleObserver>,
        identity: Arc<LifecycleIdentity>,
        started: Instant,
    ) -> Self {
        Self {
            observer,
            identity,
            started,
        }
    }

    fn emit(&self, kind: LifecycleEventKind) {
        self.observer.record(&LifecycleEvent::new(
            self.identity.clone(),
            self.started.elapsed(),
            kind,
        ));
    }
}

impl fmt::Debug for AttemptObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptObservation")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Per-attempt lifecycle and correlation inputs.
#[derive(Clone, Debug)]
pub(crate) struct AttemptContext {
    pub(crate) local_request_id: LocalRequestId,
    pub(crate) attempt_number: u32,
    pub(crate) lifecycle: RequestLifecycle,
    pub(crate) observation: Option<AttemptObservation>,
}

/// Owned response metadata, policy, and body outcome from one HTTP attempt.
pub(crate) struct AttemptResponse {
    pub(crate) plan: ResponsePlan,
    pub(crate) meta: ResponseMeta,
    pub(crate) outcome: AttemptResponseBody,
}

impl fmt::Debug for AttemptResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptResponse")
            .field("plan", &self.plan)
            .field("meta", &self.meta)
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// Success stream or bounded non-success body from one HTTP attempt.
pub(crate) enum AttemptResponseBody {
    Success(ByteStream),
    HttpFailure(LimitedBody),
}

impl fmt::Debug for AttemptResponseBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success(_) => formatter.write_str("Success(<byte stream>)"),
            Self::HttpFailure(body) => formatter.debug_tuple("HttpFailure").field(body).finish(),
        }
    }
}

/// Executes exactly one HTTP attempt without protocol-body decoding.
#[derive(Clone)]
pub(crate) struct AttemptExecutor {
    transport: Arc<dyn Transport>,
}

impl AttemptExecutor {
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }

    pub(crate) async fn execute(
        &self,
        runtime: &ProviderRuntime,
        call: PreparedCall,
        context: AttemptContext,
    ) -> Result<AttemptResponse, LlmError> {
        if context.attempt_number == 0 {
            return Err(ValidationError::new(
                "attempt_number",
                ValidationReason::Zero,
                "attempt number starts at one",
            )
            .into());
        }
        lifecycle_preflight(&context.lifecycle)?;
        let endpoint = match call.request.operation {
            ProtocolOperation::ChatCompletions => runtime.resolve_target_endpoint(&call.target)?,
        };
        emit(&context, LifecycleEventKind::EndpointResolved);

        let resolved = runtime.resolve_headers_with_protocol_operations(
            call.request.protocol_headers,
            Vec::new(),
            &call.execution.request_headers,
            Some(&call.facts),
        )?;
        let (headers, trace) = resolved.into_parts();
        emit(
            &context,
            LifecycleEventKind::HeadersResolved {
                trace: trace.into(),
            },
        );

        let request = HttpRequest::new(
            call.request.method,
            endpoint,
            headers,
            call.request.body,
            TransportContext::new(context.local_request_id.clone()),
        )
        .with_lifecycle(context.lifecycle.clone())
        .with_redirect_policy(runtime.transport_options().redirect_policy());
        emit(&context, LifecycleEventKind::TransportStarted);
        let response = self.transport.execute(request).await?;
        let (status, response_headers, body) = response.into_parts();
        let provider_request_id = provider_request_id(&response_headers);
        emit(
            &context,
            LifecycleEventKind::StatusReceived {
                status: status.as_u16(),
                provider_request_id: provider_request_id.clone(),
            },
        );

        let meta = ResponseMeta {
            local_request_id: context.local_request_id,
            provider_request_id,
            status,
            header_names: response_headers.keys().cloned().collect(),
        };
        let outcome = if status.is_success() {
            validate_success_headers(&call.response, &response_headers)?;
            AttemptResponseBody::Success(body)
        } else {
            AttemptResponseBody::HttpFailure(
                read_body_limited(body, call.response.http.max_error_body_bytes).await?,
            )
        };
        Ok(AttemptResponse {
            plan: call.response,
            meta,
            outcome,
        })
    }
}

impl fmt::Debug for AttemptExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptExecutor")
            .field("transport", &"<shared transport>")
            .finish()
    }
}

fn emit(context: &AttemptContext, kind: LifecycleEventKind) {
    if let Some(observation) = &context.observation {
        observation.emit(kind);
    }
}

fn provider_request_id(headers: &HeaderMap) -> Option<ProviderRequestId> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| ProviderRequestId::new(value).ok())
}

fn validate_success_headers(plan: &ResponsePlan, headers: &HeaderMap) -> Result<(), LlmError> {
    match plan.http.content_type {
        ExpectedContentType::EventStream => {
            let valid = headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|media_type| {
                    media_type.trim().eq_ignore_ascii_case("text/event-stream")
                });
            if !valid {
                return Err(
                    ProtocolError::new("successful response is not text/event-stream").into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use http::{HeaderMap, HeaderValue, StatusCode, header};
    use tokio::time::Instant;

    use crate::domain::{GenerateRequest, Message, ModelRef};
    use crate::execution::planner::CallPlanner;
    use crate::protocol::openai_chat::OpenAiChatDriver;
    use crate::protocol::{OpenAiChatResponsePlan, ProtocolResponsePlan};
    use crate::provider::TestOnlyProfile;
    use crate::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
    use crate::transport::{CancellationToken, RequestLifecycle};

    use super::{AttemptContext, AttemptExecutor, AttemptResponseBody};

    fn runtime(error_limit: usize) -> (crate::provider::ProviderRuntime, GenerateRequest) {
        let runtime =
            TestOnlyProfile::localhost("http://127.0.0.1:8787/v1/chat/completions", "executor-key")
                .unwrap()
                .with_max_http_error_body_bytes(error_limit)
                .unwrap()
                .build()
                .unwrap();
        let request = GenerateRequest::new(
            ModelRef::new("test-only", "gpt-test").unwrap(),
            vec![Message::user("hello")],
        );
        (runtime, request)
    }

    fn context() -> AttemptContext {
        AttemptContext {
            local_request_id: crate::domain::LocalRequestId::new("attempt-local").unwrap(),
            attempt_number: 1,
            lifecycle: RequestLifecycle::new(CancellationToken::new()),
            observation: None,
        }
    }

    #[tokio::test]
    async fn executor_owns_endpoint_headers_auth_and_success_status() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert("x-request-id", HeaderValue::from_static("provider-id"));
        let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
            StatusCode::OK,
            headers,
            Vec::new(),
        ))]);
        let (runtime, request) = runtime(64);
        let plan = CallPlanner::plan(&runtime, &request).unwrap();
        let prepared = OpenAiChatDriver.prepare(&plan).unwrap();
        let response = AttemptExecutor::new(std::sync::Arc::new(mock.clone()))
            .execute(&runtime, prepared, context())
            .await
            .unwrap();

        assert!(matches!(response.outcome, AttemptResponseBody::Success(_)));
        assert_eq!(
            response.meta.provider_request_id.unwrap().as_str(),
            "provider-id"
        );
        assert!(matches!(
            response.plan.protocol,
            ProtocolResponsePlan::OpenAiChat(OpenAiChatResponsePlan { .. })
        ));
        let captured = mock.captured_requests();
        assert_eq!(captured[0].endpoint().url().path(), "/v1/chat/completions");
        assert_eq!(
            captured[0].headers()[header::AUTHORIZATION],
            "Bearer executor-key"
        );
        assert_eq!(captured[0].headers()[header::ACCEPT], "text/event-stream");
    }

    #[tokio::test]
    async fn executor_bounds_http_failure_body_from_call_policy() {
        let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
            StatusCode::TOO_MANY_REQUESTS,
            HeaderMap::new(),
            vec![MockBodyItem::chunk(Bytes::from_static(b"0123456789"))],
        ))]);
        let (runtime, request) = runtime(4);
        let plan = CallPlanner::plan(&runtime, &request).unwrap();
        let prepared = OpenAiChatDriver.prepare(&plan).unwrap();
        let response = AttemptExecutor::new(std::sync::Arc::new(mock))
            .execute(&runtime, prepared, context())
            .await
            .unwrap();
        let AttemptResponseBody::HttpFailure(body) = response.outcome else {
            panic!("expected bounded HTTP failure");
        };
        assert_eq!(body.bytes(), &Bytes::from_static(b"0123"));
        assert!(body.is_truncated());
    }

    #[tokio::test]
    async fn expired_attempt_never_reaches_transport() {
        let mock = MockTransport::default();
        let (runtime, request) = runtime(64);
        let plan = CallPlanner::plan(&runtime, &request).unwrap();
        let prepared = OpenAiChatDriver.prepare(&plan).unwrap();
        let mut expired = context();
        expired.lifecycle = expired
            .lifecycle
            .with_deadline(Instant::now() - Duration::from_millis(1));
        let error = AttemptExecutor::new(std::sync::Arc::new(mock.clone()))
            .execute(&runtime, prepared, expired)
            .await
            .unwrap_err();
        assert!(matches!(error, crate::error::LlmError::Timeout(_)));
        assert!(mock.captured_requests().is_empty());
    }

    #[tokio::test]
    async fn each_attempt_rebuilds_headers_and_preserves_absolute_lifecycle() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let mock = MockTransport::scripted([
            MockExchange::response(MockResponse::new(
                StatusCode::OK,
                headers.clone(),
                Vec::new(),
            )),
            MockExchange::response(MockResponse::new(StatusCode::OK, headers, Vec::new())),
        ]);
        let (runtime, request) = runtime(64);
        let plan = CallPlanner::plan(&runtime, &request).unwrap();
        let prepared = OpenAiChatDriver.prepare(&plan).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let executor = AttemptExecutor::new(std::sync::Arc::new(mock.clone()));
        for attempt_number in 1..=2 {
            executor
                .execute(
                    &runtime,
                    prepared.clone(),
                    AttemptContext {
                        local_request_id: crate::domain::LocalRequestId::new(format!(
                            "attempt-{attempt_number}"
                        ))
                        .unwrap(),
                        attempt_number,
                        lifecycle: RequestLifecycle::new(CancellationToken::new())
                            .with_deadline(deadline),
                        observation: None,
                    },
                )
                .await
                .unwrap();
        }
        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 2);
        assert_ne!(
            captured[0].local_request_id(),
            captured[1].local_request_id()
        );
        assert_eq!(captured[0].deadline(), Some(deadline));
        assert_eq!(captured[1].deadline(), Some(deadline));
        assert_eq!(
            captured[0].headers()[header::AUTHORIZATION],
            "Bearer executor-key"
        );
        assert_eq!(
            captured[1].headers()[header::AUTHORIZATION],
            "Bearer executor-key"
        );
    }
}
