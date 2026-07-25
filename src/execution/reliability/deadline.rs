//! Deadline queries shared by retry and attempt orchestration.

use std::time::Duration;

use tokio::time::Instant;

use crate::transport::RequestLifecycle;

pub(crate) fn remaining(lifecycle: &RequestLifecycle) -> Option<Duration> {
    lifecycle.remaining(Instant::now())
}

#[cfg(test)]
pub(crate) fn can_start_attempt(lifecycle: &RequestLifecycle, minimum_budget: Duration) -> bool {
    remaining(lifecycle).is_none_or(|remaining| remaining >= minimum_budget)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use crate::transport::{CancellationToken, RequestLifecycle};

    use super::can_start_attempt;

    #[test]
    fn minimum_attempt_budget_uses_the_existing_absolute_deadline() {
        let lifecycle = RequestLifecycle::new(CancellationToken::new())
            .with_deadline(Instant::now() + Duration::from_secs(1));
        assert!(can_start_attempt(&lifecycle, Duration::from_millis(500)));
        assert!(!can_start_attempt(&lifecycle, Duration::from_secs(2)));
    }
}
