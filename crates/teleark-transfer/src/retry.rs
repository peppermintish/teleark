use teleark_core::TransferError;

use crate::{Clock, ConfigurationError, JitterSource, TransferEngineError};

/// Bounded retry/backoff policy. `max_attempts` includes the initial attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
    max_jitter_ms: u64,
}

impl RetryPolicy {
    pub fn new(
        max_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
        max_jitter_ms: u64,
    ) -> Result<Self, ConfigurationError> {
        if max_attempts == 0 {
            return Err(ConfigurationError::InvalidRetryPolicy {
                field: "max_attempts",
            });
        }
        if base_delay_ms == 0 {
            return Err(ConfigurationError::InvalidRetryPolicy {
                field: "base_delay_ms",
            });
        }
        if max_delay_ms < base_delay_ms {
            return Err(ConfigurationError::InvalidRetryPolicy {
                field: "max_delay_ms",
            });
        }
        Ok(Self {
            max_attempts,
            base_delay_ms,
            max_delay_ms,
            max_jitter_ms,
        })
    }

    /// Decide using an explicit clock and deterministic/injected jitter source.
    pub fn decide(
        self,
        error: &TransferError,
        completed_attempts: u32,
        clock: &impl Clock,
        jitter: &mut impl JitterSource,
    ) -> Result<RetryDecision, TransferEngineError> {
        if !error.is_retryable() || completed_attempts >= self.max_attempts {
            return Ok(RetryDecision::GiveUp);
        }
        let exponent = completed_attempts.saturating_sub(1).min(63);
        let exponential = self
            .base_delay_ms
            .saturating_mul(1_u64 << exponent)
            .min(self.max_delay_ms);
        let jitter_ms = jitter
            .jitter_millis(self.max_jitter_ms)
            .min(self.max_jitter_ms);
        let backoff = exponential.saturating_add(jitter_ms);
        let server_delay = match error {
            TransferError::FloodWait { retry_after } => {
                u64::try_from(retry_after.as_millis()).unwrap_or(u64::MAX)
            }
            _ => 0,
        };
        let delay_ms = backoff.max(server_delay);
        let not_before_ms = clock.now_millis().checked_add(delay_ms).ok_or(
            TransferEngineError::ArithmeticOverflow {
                field: "retry deadline",
            },
        )?;
        Ok(RetryDecision::RetryAt {
            next_attempt: completed_attempts + 1,
            not_before_ms,
            delay_ms,
        })
    }
}

/// Machine-readable result of retry classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDecision {
    RetryAt {
        next_attempt: u32,
        not_before_ms: u64,
        delay_ms: u64,
    },
    GiveUp,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    struct TestClock(u64);

    impl Clock for TestClock {
        fn now_millis(&self) -> u64 {
            self.0
        }
    }

    struct TestJitter(u64);

    impl JitterSource for TestJitter {
        fn jitter_millis(&mut self, _maximum_millis: u64) -> u64 {
            self.0
        }
    }

    fn policy() -> RetryPolicy {
        match RetryPolicy::new(3, 100, 1_000, 25) {
            Ok(value) => value,
            Err(error) => panic!("test retry config failed: {error}"),
        }
    }

    #[test]
    fn exponential_jitter_is_deterministic_and_bounded() {
        let decision = policy().decide(
            &TransferError::Network,
            2,
            &TestClock(1_000),
            &mut TestJitter(10),
        );
        assert_eq!(
            decision,
            Ok(RetryDecision::RetryAt {
                next_attempt: 3,
                not_before_ms: 1_210,
                delay_ms: 210,
            })
        );
        assert_eq!(
            policy().decide(
                &TransferError::Network,
                3,
                &TestClock(0),
                &mut TestJitter(0),
            ),
            Ok(RetryDecision::GiveUp)
        );
    }

    #[test]
    fn flood_wait_is_structured_and_never_shortened_by_backoff() {
        let decision = policy().decide(
            &TransferError::FloodWait {
                retry_after: Duration::from_millis(750),
            },
            1,
            &TestClock(50),
            &mut TestJitter(0),
        );
        assert_eq!(
            decision,
            Ok(RetryDecision::RetryAt {
                next_attempt: 2,
                not_before_ms: 800,
                delay_ms: 750,
            })
        );
    }

    #[test]
    fn permanent_error_never_retries() {
        assert_eq!(
            policy().decide(
                &TransferError::SourceChanged,
                1,
                &TestClock(0),
                &mut TestJitter(0),
            ),
            Ok(RetryDecision::GiveUp)
        );
    }
}
