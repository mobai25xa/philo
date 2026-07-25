//! Monotonic logical-request and public-delivery state.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryState {
    NothingDelivered,
    DomainEventDelivered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum RequestProgress {
    Planned = 0,
    Attempting = 1,
    Streaming = 2,
    Terminal = 3,
}

#[derive(Debug)]
pub(crate) struct RequestExecutionState {
    progress: AtomicU8,
    delivered: AtomicBool,
    terminal: AtomicBool,
}

impl RequestExecutionState {
    pub(crate) fn new() -> Self {
        Self {
            progress: AtomicU8::new(RequestProgress::Planned as u8),
            delivered: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
        }
    }

    pub(crate) fn begin_attempt(&self) {
        if !self.terminal.load(Ordering::Acquire) {
            self.progress
                .store(RequestProgress::Attempting as u8, Ordering::Release);
        }
    }

    pub(crate) fn mark_delivered(&self) {
        self.delivered.store(true, Ordering::Release);
        if !self.terminal.load(Ordering::Acquire) {
            self.progress
                .store(RequestProgress::Streaming as u8, Ordering::Release);
        }
    }

    pub(crate) fn delivery_state(&self) -> DeliveryState {
        if self.delivered.load(Ordering::Acquire) {
            DeliveryState::DomainEventDelivered
        } else {
            DeliveryState::NothingDelivered
        }
    }

    pub(crate) fn mark_terminal(&self) -> bool {
        if self
            .terminal
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.progress
                .store(RequestProgress::Terminal as u8, Ordering::Release);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DeliveryState, RequestExecutionState};

    #[test]
    fn delivery_is_monotonic_and_terminal_is_unique() {
        let state = RequestExecutionState::new();
        assert_eq!(state.delivery_state(), DeliveryState::NothingDelivered);
        state.begin_attempt();
        state.mark_delivered();
        state.begin_attempt();
        assert_eq!(state.delivery_state(), DeliveryState::DomainEventDelivered);
        assert!(state.mark_terminal());
        assert!(!state.mark_terminal());
    }
}
