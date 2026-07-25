//! One-attempt endpoint, header, authentication, and HTTP execution.

use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use http::{HeaderMap, header};
use tokio::time::Instant;

use crate::domain::{LocalRequestId, ProviderRequestId};
use crate::error::{LlmError, ProtocolError, ValidationError, ValidationReason};
use crate::observability::{
    AttemptIdentity, LifecycleEvent, LifecycleEventKind, LifecycleIdentity, LifecycleObserver,
};
use crate::protocol::{
    ExpectedContentType, PreparedCall, ProtocolOperation, ResponseMeta, ResponsePlan,
};
use crate::provider::runtime::HeaderAttemptContext;
use crate::provider::{
    ProviderRuntime, RateLimitHeaderKind, RateLimitValue, ResolvedIdempotency, observe_rate_limit,
};
use crate::transport::{
    ByteStream, HttpRequest, LimitedBody, RequestLifecycle, Transport, TransportContext,
    await_with_stage, lifecycle_preflight, read_body_limited,
};

use super::reliability::{RetryAfterHeader, TimeoutPolicy, parse_retry_after};

const MAX_RESPONSE_HEADER_FIELDS: usize = 128;
const MAX_RESPONSE_HEADER_VALUE_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_HEADER_TOTAL_BYTES: usize = 64 * 1024;

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

    pub(crate) fn emit(&self, kind: LifecycleEventKind) {
        crate::observability::trace::record_safely(
            &*self.observer,
            &LifecycleEvent::new(self.identity.clone(), self.started.elapsed(), kind),
        );
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
    pub(crate) attempt: AttemptIdentity,
    pub(crate) lifecycle: RequestLifecycle,
    pub(crate) timeouts: TimeoutPolicy,
    pub(crate) observation: Option<AttemptObservation>,
    pub(crate) idempotency: ResolvedIdempotency,
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

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn execute(
        &self,
        runtime: &ProviderRuntime,
        call: PreparedCall,
        context: AttemptContext,
    ) -> Result<AttemptResponse, LlmError> {
        if context.attempt.number() == 0 {
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

        let resolved = await_with_stage(
            &context.lifecycle,
            crate::error::TimeoutStage::Credential,
            Some(context.timeouts.credential_timeout()),
            Some(context.attempt.number()),
            false,
            runtime.resolve_headers_for_attempt(
                call.request.protocol_headers,
                Vec::new(),
                context.idempotency.operation()?.into_iter().collect(),
                &call.execution.request_headers,
                HeaderAttemptContext {
                    endpoint: &endpoint,
                    facts: &call.facts,
                    lifecycle: &context.lifecycle,
                    model_id: &call.target.domain_model,
                    local_request_id: &context.local_request_id,
                    attempt_number: context.attempt.number(),
                },
            ),
        )
        .await??;
        emit(
            &context,
            LifecycleEventKind::CredentialResolved {
                attempt: context.attempt.clone(),
            },
        );
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
        let response = await_with_stage(
            &context.lifecycle,
            crate::error::TimeoutStage::ResponseHeader,
            Some(context.timeouts.response_header_timeout()),
            Some(context.attempt.number()),
            false,
            self.transport.execute(request),
        )
        .await??;
        let (status, response_headers, body) = response.into_parts();
        validate_response_header_limits(&response_headers)?;
        let provider_request_id = provider_request_id(&response_headers);
        emit(
            &context,
            LifecycleEventKind::StatusReceived {
                status: status.as_u16(),
                provider_request_id: provider_request_id.clone(),
            },
        );
        let rate_limit = observed_rate_limit(status, &response_headers, runtime, &context);
        let retry_after = rate_limit.retry_after_delay();

        let meta = ResponseMeta {
            local_request_id: context.local_request_id,
            provider_request_id,
            status,
            header_names: response_headers.keys().cloned().collect(),
            retry_after,
            rate_limit,
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

fn observed_rate_limit(
    status: http::StatusCode,
    headers: &HeaderMap,
    runtime: &ProviderRuntime,
    context: &AttemptContext,
) -> crate::provider::RateLimitObservation {
    let now = SystemTime::now();
    let mut declarations = vec![RetryAfterHeader::Standard];
    declarations.extend(
        runtime
            .rate_limit_policy()
            .headers()
            .iter()
            .filter_map(|spec| match spec.kind() {
                RateLimitHeaderKind::RetryAfterSeconds => {
                    Some(RetryAfterHeader::ProviderDeltaSeconds(spec.name().clone()))
                }
                RateLimitHeaderKind::RetryAtUnixSeconds => {
                    Some(RetryAfterHeader::ProviderUnixSeconds(spec.name().clone()))
                }
                RateLimitHeaderKind::RemainingRequests
                | RateLimitHeaderKind::RemainingUnits(_)
                | RateLimitHeaderKind::ResetAfterSeconds
                | RateLimitHeaderKind::ResetAtUnixSeconds => None,
            }),
    );
    let retry = parse_retry_after(headers, &declarations, now);
    let retry_after = match (retry.present, retry.valid, retry.delay) {
        (false, _, _) => RateLimitValue::Unknown,
        (true, true, Some(delay)) => RateLimitValue::Valid(delay),
        (true, _, _) => RateLimitValue::Invalid,
    };
    let observation = observe_rate_limit(
        status,
        headers,
        runtime.rate_limit_policy(),
        retry_after,
        now,
    );
    let provider_fields_present =
        !matches!(observation.remaining_requests(), RateLimitValue::Unknown)
            || !matches!(observation.remaining_units(), RateLimitValue::Unknown)
            || !matches!(observation.reset(), RateLimitValue::Unknown);
    if retry.present || provider_fields_present || observation.status_is_rate_limited() {
        emit(
            context,
            LifecycleEventKind::RateLimitObserved {
                status_is_rate_limited: observation.status_is_rate_limited(),
                retry_after_valid: matches!(observation.retry_after(), RateLimitValue::Valid(_)),
                provider_fields_present,
            },
        );
    }
    observation
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

fn validate_response_header_limits(headers: &HeaderMap) -> Result<(), LlmError> {
    let mut fields = 0_usize;
    let mut total_bytes = 0_usize;
    for (name, value) in headers {
        fields = fields.saturating_add(1);
        let value_bytes = value.as_bytes().len();
        total_bytes = total_bytes
            .saturating_add(name.as_str().len())
            .saturating_add(value_bytes);
        if fields > MAX_RESPONSE_HEADER_FIELDS
            || value_bytes > MAX_RESPONSE_HEADER_VALUE_BYTES
            || total_bytes > MAX_RESPONSE_HEADER_TOTAL_BYTES
        {
            return Err(ProtocolError::new("response header resource limit exceeded").into());
        }
    }
    Ok(())
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
    use crate::execution::reliability::TimeoutPolicy;
    use crate::observability::{AttemptId, AttemptIdentity};
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
            attempt: AttemptIdentity::new(AttemptId::new("attempt-1".to_owned()), 1),
            lifecycle: RequestLifecycle::new(CancellationToken::new()),
            timeouts: TimeoutPolicy::default(),
            observation: None,
            idempotency: crate::provider::ResolvedIdempotency::resolve(
                &crate::provider::IdempotencyPolicy::standard_header(),
                None,
                false,
                false,
            )
            .unwrap(),
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
        let local_request_id = crate::domain::LocalRequestId::new("logical-request").unwrap();
        for attempt_number in 1..=2 {
            executor
                .execute(
                    &runtime,
                    prepared.clone(),
                    AttemptContext {
                        local_request_id: local_request_id.clone(),
                        attempt: AttemptIdentity::new(
                            AttemptId::new(format!("attempt-{attempt_number}")),
                            attempt_number,
                        ),
                        lifecycle: RequestLifecycle::new(CancellationToken::new())
                            .with_deadline(deadline),
                        timeouts: TimeoutPolicy::default(),
                        observation: None,
                        idempotency: crate::provider::ResolvedIdempotency::resolve(
                            &crate::provider::IdempotencyPolicy::standard_header(),
                            None,
                            false,
                            false,
                        )
                        .unwrap(),
                    },
                )
                .await
                .unwrap();
        }
        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 2);
        assert_eq!(
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
