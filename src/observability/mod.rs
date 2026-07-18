//! Value-free lifecycle diagnostics for SDK requests.

mod trace;

pub use trace::{
    LifecycleErrorCategory, LifecycleEvent, LifecycleEventKind, LifecycleIdentity,
    LifecycleObserver,
};
