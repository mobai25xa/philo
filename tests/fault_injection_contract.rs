//! Deterministic request fault-injection contracts.

mod support;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::{GenerateRequest, LlmClient, LlmError, Message, ModelRef, RequestControl};
use support::mock_transport::{MockBodyItem, MockExchange, MockGate, MockResponse, MockTransport};
use support::provider::TestProvider;

const ENDPOINT: &str = "https://test.invalid/v1/chat/completions";

struct FaultCase {
    name: &'static str,
    trigger: &'static str,
    attempts: &'static str,
    terminal: &'static str,
    cleanup: &'static str,
    reason: &'static str,
}

const FAULT_MATRIX: &[FaultCase] = &[
    FaultCase {
        name: "connect",
        trigger: "transport start",
        attempts: "bounded retry",
        terminal: "success or connect error",
        cleanup: "no body",
        reason: "ConnectFailure",
    },
    FaultCase {
        name: "credential",
        trigger: "credential resolve",
        attempts: "no doomed attempt",
        terminal: "credential/timeout",
        cleanup: "no transport",
        reason: "CredentialTimeout",
    },
    FaultCase {
        name: "response-header",
        trigger: "response header wait",
        attempts: "bounded retry",
        terminal: "timeout",
        cleanup: "request future dropped",
        reason: "StageTimeout",
    },
    FaultCase {
        name: "first-event",
        trigger: "first domain event wait",
        attempts: "bounded retry",
        terminal: "timeout or recovery",
        cleanup: "failed body dropped",
        reason: "StageTimeout",
    },
    FaultCase {
        name: "idle-stream",
        trigger: "post-delivery idle wait",
        attempts: "one",
        terminal: "timeout",
        cleanup: "body cancelled",
        reason: "DeliveryBoundaryClosed",
    },
    FaultCase {
        name: "rate-limit",
        trigger: "HTTP 429",
        attempts: "bounded retry",
        terminal: "success or 429",
        cleanup: "error body bounded",
        reason: "RateLimited",
    },
    FaultCase {
        name: "transient-status",
        trigger: "HTTP 502-504",
        attempts: "replay-safe only",
        terminal: "success or status",
        cleanup: "error body bounded",
        reason: "TransientServerError",
    },
    FaultCase {
        name: "sse-limit",
        trigger: "frame/line/chunk limit",
        attempts: "before-delivery only",
        terminal: "protocol",
        cleanup: "decoder terminal",
        reason: "NonRetryable",
    },
    FaultCase {
        name: "malformed-json",
        trigger: "JSON decode",
        attempts: "one",
        terminal: "protocol",
        cleanup: "state terminal",
        reason: "NonRetryable",
    },
    FaultCase {
        name: "drop",
        trigger: "public stream drop",
        attempts: "no future attempt",
        terminal: "cancelled observation",
        cleanup: "body cancelled",
        reason: "Cancelled",
    },
    FaultCase {
        name: "observer-panic",
        trigger: "synchronous hook",
        attempts: "unchanged",
        terminal: "main result unchanged",
        cleanup: "normal",
        reason: "Isolated",
    },
    FaultCase {
        name: "overall-deadline",
        trigger: "absolute deadline",
        attempts: "no reset",
        terminal: "timeout",
        cleanup: "body/timer dropped",
        reason: "DeadlineInsufficient",
    },
];

#[test]
fn every_fault_case_declares_trigger_attempt_terminal_cleanup_and_reason() {
    assert!(FAULT_MATRIX.len() >= 12);
    for case in FAULT_MATRIX {
        assert!(!case.name.is_empty());
        assert!(!case.trigger.is_empty());
        assert!(!case.attempts.is_empty());
        assert!(!case.terminal.is_empty());
        assert!(!case.cleanup.is_empty());
        assert!(!case.reason.is_empty());
    }
}

#[tokio::test]
async fn deterministic_gate_proves_cancel_releases_a_blocked_body() {
    let gate = MockGate::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers,
        vec![
            MockBodyItem::chunk(Bytes::from_static(b"data: [DONE]\n\n")).behind_gate(gate.clone()),
        ],
    ))]);
    let runtime = TestProvider::new(ENDPOINT, "fault-fixture-key")
        .unwrap()
        .build()
        .unwrap();
    let control = RequestControl::new();
    let cancel = control.clone();
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("fixed fault fixture")],
    );
    let mut stream = LlmClient::new(runtime, mock.clone())
        .stream_with_control(request, control)
        .await
        .unwrap();
    let next = stream.next();
    tokio::pin!(next);
    loop {
        tokio::select! {
            result = &mut next => panic!("blocked gate returned unexpectedly: {result:?}"),
            () = tokio::task::yield_now() => {
                if mock.body_poll_count() > 0 {
                    cancel.cancel();
                    break;
                }
            }
        }
    }
    let result = next.await.unwrap();
    assert!(matches!(result, Err(LlmError::Cancelled)));
    assert!(!gate.is_open());
    assert_eq!(mock.body_cancellation_count(), 1);
    assert_eq!(mock.remaining_expectations(), 0);
}
