use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use tokio::time::Instant;
use uuid::Uuid;

use crate::domain::{
    AssistantEvent, AssistantMessage, GenerateRequest, LocalRequestId, RequestTimeout, TraceId,
    collect_assistant_message_for_format,
};
use crate::error::{
    ErrorStage, LlmError, TimeoutError, TruncatedStreamError, ValidationError, ValidationReason,
};
use crate::execution::executor::{AttemptContext, AttemptExecutor, AttemptObservation};
use crate::execution::planner::CallPlanner;
use crate::observability::{
    LifecycleErrorCategory, LifecycleEvent, LifecycleEventKind, LifecycleIdentity,
    LifecycleObserver,
};
use crate::protocol::{ProtocolDispatch, ResponseSession};
use crate::provider::ProviderRuntime;
use crate::transport::{CancellationToken, RequestLifecycle, ReqwestTransport, Transport};

type EventStream = Pin<Box<dyn Stream<Item = Result<AssistantEvent, LlmError>> + Send + 'static>>;

/// Caller-owned controls that may be retained while `stream()` waits for headers.
#[derive(Clone, Debug, Default)]
pub struct RequestControl {
    cancellation: CancellationToken,
    trace_id: Option<TraceId>,
}

impl RequestControl {
    /// Creates controls with a fresh cancellation token and no telemetry ID.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches caller telemetry correlation without sending it to the provider.
    #[must_use]
    pub fn with_trace_id(mut self, trace_id: TraceId) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Cancels connection, response, and body work using this control.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Returns the shared cancellation token.
    #[must_use]
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Returns caller telemetry correlation, when configured.
    #[must_use]
    pub fn trace_id(&self) -> Option<&TraceId> {
        self.trace_id.as_ref()
    }
}

/// Streaming assistant events with cancellation-on-drop semantics.
pub struct AssistantStream {
    inner: EventStream,
    cancellation: CancellationToken,
    local_request_id: LocalRequestId,
    observation: Option<Observation>,
    lifecycle_state: StreamLifecycleState,
    completion_state: StreamCompletionState,
}

#[derive(Debug, Default)]
struct StreamLifecycleState {
    first_event_seen: bool,
    terminal: bool,
}

#[derive(Debug, Default)]
struct StreamCompletionState {
    partial_output: bool,
    usage_known: bool,
    event_count: u64,
}

impl AssistantStream {
    fn new(
        inner: EventStream,
        cancellation: CancellationToken,
        local_request_id: LocalRequestId,
        observation: Option<Observation>,
    ) -> Self {
        Self {
            inner,
            cancellation,
            local_request_id,
            observation,
            lifecycle_state: StreamLifecycleState::default(),
            completion_state: StreamCompletionState::default(),
        }
    }

    /// Returns the SDK-generated ID for this call.
    #[must_use]
    pub fn local_request_id(&self) -> &LocalRequestId {
        &self.local_request_id
    }

    /// Requests cancellation of the remaining stream.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Returns the cancellation token shared with the transport body.
    #[must_use]
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation
    }

    fn observe_first(&mut self, event: &AssistantEvent) {
        if self.lifecycle_state.first_event_seen {
            return;
        }
        self.lifecycle_state.first_event_seen = true;
        let (provider_request_id, generation_id) = match event {
            AssistantEvent::Start {
                provider_request_id,
                generation_id,
                ..
            } => (provider_request_id.clone(), generation_id.clone()),
            _ => (None, None),
        };
        emit(
            self.observation.as_ref(),
            LifecycleEventKind::FirstSseEvent {
                provider_request_id,
                generation_id,
            },
        );
    }

    fn observe_error(&self, error: &LlmError) {
        emit_terminal_error(
            self.observation.as_ref(),
            error,
            self.completion_state.partial_output,
        );
    }
}

impl fmt::Debug for AssistantStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssistantStream")
            .field("local_request_id", &self.local_request_id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("lifecycle_state", &self.lifecycle_state)
            .field("completion_state", &self.completion_state)
            .finish_non_exhaustive()
    }
}

impl Stream for AssistantStream {
    type Item = Result<AssistantEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        if stream.lifecycle_state.terminal {
            return Poll::Ready(None);
        }
        match stream.inner.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(event))) => {
                stream.observe_first(&event);
                stream.completion_state.event_count =
                    stream.completion_state.event_count.saturating_add(1);
                match &event {
                    AssistantEvent::TextDelta { delta, .. } if !delta.is_empty() => {
                        stream.completion_state.partial_output = true;
                    }
                    AssistantEvent::Usage(_) | AssistantEvent::DetailedUsage(_) => {
                        stream.completion_state.usage_known = true;
                    }
                    AssistantEvent::Done { finish_reason } => {
                        emit(
                            stream.observation.as_ref(),
                            LifecycleEventKind::FinishSeen {
                                finish_reason: finish_reason.clone(),
                            },
                        );
                        emit(stream.observation.as_ref(), LifecycleEventKind::DoneSeen);
                        emit(
                            stream.observation.as_ref(),
                            LifecycleEventKind::RequestCompleted {
                                event_count: stream.completion_state.event_count,
                                usage_known: stream.completion_state.usage_known,
                            },
                        );
                        stream.lifecycle_state.terminal = true;
                    }
                    _ => {}
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                stream.lifecycle_state.terminal = true;
                stream.observe_error(&error);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                stream.lifecycle_state.terminal = true;
                let error = LlmError::from(TruncatedStreamError);
                stream.observe_error(&error);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AssistantStream {
    fn drop(&mut self) {
        if !self.lifecycle_state.terminal {
            self.cancellation.cancel();
            emit(
                self.observation.as_ref(),
                LifecycleEventKind::RequestCancelled {
                    partial_output: self.completion_state.partial_output,
                },
            );
        }
    }
}

/// Immutable, concurrency-safe phase-one client.
#[derive(Clone)]
pub struct LlmClient {
    runtime: Arc<ProviderRuntime>,
    transport: Arc<dyn Transport>,
    observer: Option<Arc<dyn LifecycleObserver>>,
}

impl LlmClient {
    /// Creates a client from an immutable provider runtime and transport.
    pub fn new<T>(runtime: ProviderRuntime, transport: T) -> Self
    where
        T: Transport + 'static,
    {
        Self {
            runtime: Arc::new(runtime),
            transport: Arc::new(transport),
            observer: None,
        }
    }

    /// Creates a client from already shared runtime and transport objects.
    pub fn from_shared(runtime: Arc<ProviderRuntime>, transport: Arc<dyn Transport>) -> Self {
        Self {
            runtime,
            transport,
            observer: None,
        }
    }

    /// Creates a client using the production shared reqwest transport.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the production HTTP client cannot be built.
    pub fn with_reqwest(runtime: ProviderRuntime) -> Result<Self, LlmError> {
        Ok(Self::new(runtime, ReqwestTransport::new()?))
    }

    /// Installs a synchronous, value-free lifecycle observer.
    #[must_use]
    pub fn with_observer<O>(mut self, observer: O) -> Self
    where
        O: LifecycleObserver + 'static,
    {
        self.observer = Some(Arc::new(observer));
        self
    }

    /// Installs an already shared lifecycle observer.
    #[must_use]
    pub fn with_shared_observer(mut self, observer: Arc<dyn LifecycleObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Returns the immutable provider runtime.
    #[must_use]
    pub fn runtime(&self) -> &ProviderRuntime {
        &self.runtime
    }

    /// Starts one request using fresh default controls.
    ///
    /// # Errors
    ///
    /// Returns a typed validation, configuration, transport, HTTP status, or lifecycle error
    /// before the response body stream is available.
    pub async fn stream(&self, request: GenerateRequest) -> Result<AssistantStream, LlmError> {
        self.stream_with_control(request, RequestControl::new())
            .await
    }

    /// Starts one request using caller-retained cancellation and telemetry controls.
    ///
    /// # Errors
    ///
    /// Returns a typed validation, configuration, transport, HTTP status, cancellation, or
    /// deadline error before the response body stream is available.
    pub async fn stream_with_control(
        &self,
        request: GenerateRequest,
        control: RequestControl,
    ) -> Result<AssistantStream, LlmError> {
        let local_request_id = LocalRequestId::new(Uuid::new_v4().to_string())?;
        let observation = self.observation(&request, &control, local_request_id.clone());
        emit(observation.as_ref(), LifecycleEventKind::RequestStarted);

        let result = self
            .start_stream(
                &request,
                &control,
                local_request_id.clone(),
                observation.as_ref(),
            )
            .await;
        match result {
            Ok(inner) => Ok(AssistantStream::new(
                inner,
                control.cancellation,
                local_request_id,
                observation,
            )),
            Err(error) => {
                emit_terminal_error(observation.as_ref(), &error, false);
                Err(error)
            }
        }
    }

    /// Completes one request by consuming [`Self::stream`] exactly once.
    ///
    /// # Errors
    ///
    /// Returns the first error from request startup or streamed response processing.
    pub async fn complete(&self, request: GenerateRequest) -> Result<AssistantMessage, LlmError> {
        let model = request.model().clone();
        let response_format = request.options().response_format().clone();
        let stream = self.stream(request).await?;
        Ok(
            collect_assistant_message_for_format(stream, &response_format)
                .await?
                .with_model(model),
        )
    }

    /// Completes one request using caller-retained cancellation and telemetry controls.
    ///
    /// # Errors
    ///
    /// Returns the first error from request startup or streamed response processing.
    pub async fn complete_with_control(
        &self,
        request: GenerateRequest,
        control: RequestControl,
    ) -> Result<AssistantMessage, LlmError> {
        let model = request.model().clone();
        let response_format = request.options().response_format().clone();
        let stream = self.stream_with_control(request, control).await?;
        Ok(
            collect_assistant_message_for_format(stream, &response_format)
                .await?
                .with_model(model),
        )
    }

    async fn start_stream(
        &self,
        request: &GenerateRequest,
        control: &RequestControl,
        local_request_id: LocalRequestId,
        observation: Option<&Observation>,
    ) -> Result<EventStream, LlmError> {
        let lifecycle = request_lifecycle(request, control.cancellation.clone())?;
        lifecycle_preflight(&lifecycle)?;

        let plan = CallPlanner::plan(&self.runtime, request)?;
        emit(observation, LifecycleEventKind::ValidationCompleted);

        let driver = ProtocolDispatch::for_kind(plan.policy.target.protocol_kind);
        let prepared = driver.prepare(&plan)?;
        let executor = AttemptExecutor::new(self.transport.clone());
        let response = executor
            .execute(
                &self.runtime,
                prepared,
                AttemptContext {
                    local_request_id,
                    attempt_number: 1,
                    lifecycle,
                    observation: observation.map(Observation::attempt_observation),
                },
            )
            .await?;
        ResponseSession::open(response)
    }

    fn observation(
        &self,
        request: &GenerateRequest,
        control: &RequestControl,
        local_request_id: LocalRequestId,
    ) -> Option<Observation> {
        let observer = self.observer.clone()?;
        Some(Observation {
            observer,
            identity: Arc::new(LifecycleIdentity::new(
                local_request_id,
                control.trace_id.clone(),
                self.runtime.provider_id().clone(),
                request.model().model().clone(),
                self.runtime.protocol_id().clone(),
            )),
            started: Instant::now(),
        })
    }
}

impl fmt::Debug for LlmClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmClient")
            .field("runtime", &self.runtime)
            .field("transport", &"<shared transport>")
            .field("observer_enabled", &self.observer.is_some())
            .finish()
    }
}

struct Observation {
    observer: Arc<dyn LifecycleObserver>,
    identity: Arc<LifecycleIdentity>,
    started: Instant,
}

impl Observation {
    fn attempt_observation(&self) -> AttemptObservation {
        AttemptObservation::new(self.observer.clone(), self.identity.clone(), self.started)
    }
}

fn emit(observation: Option<&Observation>, kind: LifecycleEventKind) {
    if let Some(observation) = observation {
        observation.observer.record(&LifecycleEvent::new(
            Arc::clone(&observation.identity),
            observation.started.elapsed(),
            kind,
        ));
    }
}

fn emit_terminal_error(observation: Option<&Observation>, error: &LlmError, partial_output: bool) {
    let kind = match error {
        LlmError::Cancelled => LifecycleEventKind::RequestCancelled { partial_output },
        LlmError::Timeout(_) => LifecycleEventKind::RequestTimedOut { partial_output },
        _ => LifecycleEventKind::RequestFailed {
            category: LifecycleErrorCategory::from_error(error),
            partial_output,
        },
    };
    emit(observation, kind);
}

fn request_lifecycle(
    request: &GenerateRequest,
    cancellation: CancellationToken,
) -> Result<RequestLifecycle, LlmError> {
    let deadline = match request.options().timeout() {
        None => None,
        Some(RequestTimeout::At(deadline)) => Some(deadline),
        Some(RequestTimeout::After(duration)) => {
            Some(Instant::now().checked_add(duration).ok_or_else(|| {
                ValidationError::new(
                    "timeout",
                    ValidationReason::Overflow,
                    "relative timeout cannot be represented as an absolute deadline",
                )
            })?)
        }
    };
    let lifecycle = RequestLifecycle::new(cancellation);
    Ok(match deadline {
        Some(deadline) => lifecycle.with_deadline(deadline),
        None => lifecycle,
    })
}

fn lifecycle_preflight(lifecycle: &RequestLifecycle) -> Result<(), LlmError> {
    if lifecycle.cancellation().is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    if lifecycle
        .deadline()
        .is_some_and(|deadline| deadline <= Instant::now())
    {
        return Err(TimeoutError::new(ErrorStage::Timeout).into());
    }
    Ok(())
}
