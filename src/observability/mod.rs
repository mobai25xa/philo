//! Value-free lifecycle diagnostics for SDK requests.

pub(crate) mod trace;
#[cfg(feature = "tracing")]
mod tracing_adapter;

pub use trace::{
    AttemptId, AttemptIdentity, LifecycleErrorCategory, LifecycleEvent, LifecycleEventKind,
    LifecycleIdentity, LifecycleObserver, RetryStopReason,
};
#[cfg(feature = "tracing")]
pub use tracing_adapter::TracingObserver;
