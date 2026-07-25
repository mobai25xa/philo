//! Shared request reliability policy, state, and decisions.

mod deadline;
mod lifecycle;
mod retry;
mod timeout;
mod wait;

pub use retry::RetryPolicy;
pub use timeout::TimeoutPolicy;
pub use wait::RetryWaitPolicy;

pub(crate) use deadline::remaining;
pub(crate) use lifecycle::{DeliveryState, RequestExecutionState};
pub(crate) use retry::{RetryDecision, decide_retry};
pub(crate) use wait::{RetryAfterHeader, calculate_retry_wait, parse_retry_after, wait_for_retry};
