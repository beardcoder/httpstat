//! A wall-clock budget for one request.
//!
//! `--max-time` used to be installed as a socket read timeout, which resets on
//! every successful read — a slow trickle of bytes could keep a request alive
//! indefinitely. A deadline is an absolute point in time instead, so the budget
//! covers the whole operation including redirects.

use std::time::{Duration, Instant};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    at: Option<Instant>,
    limit: Option<Duration>,
}

impl Deadline {
    /// A budget of `limit` starting now; `None` means unlimited.
    pub fn new(limit: Option<Duration>) -> Self {
        Deadline {
            at: limit.map(|d| Instant::now() + d),
            limit,
        }
    }

    /// An unlimited budget.
    pub fn unlimited() -> Self {
        Deadline {
            at: None,
            limit: None,
        }
    }

    /// Time left, or `None` when unlimited. `Some(ZERO)` means it has expired.
    pub fn remaining(&self) -> Option<Duration> {
        self.at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    pub fn expired(&self) -> bool {
        self.remaining().is_some_and(|left| left.is_zero())
    }

    /// The error to report when the budget runs out during `phase`.
    pub fn timeout_error(&self, phase: &str) -> Error {
        match self.limit {
            Some(limit) => Error::request(format!(
                "timed out after {:.3}s during {phase} (--max-time)",
                limit.as_secs_f64()
            )),
            None => Error::request(format!("timed out during {phase}")),
        }
    }

    /// Fail if the budget is already spent.
    pub fn check(&self, phase: &str) -> Result<()> {
        if self.expired() {
            return Err(self.timeout_error(phase));
        }
        Ok(())
    }

    /// The shorter of the remaining budget and `other`, for a per-operation
    /// timeout such as `--connect-timeout`.
    pub fn cap(&self, other: Option<Duration>) -> Option<Duration> {
        match (self.remaining(), other) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlimited_deadline_never_expires() {
        let d = Deadline::unlimited();
        assert_eq!(d.remaining(), None);
        assert!(!d.expired());
        assert!(d.check("connect").is_ok());
        assert_eq!(
            d.cap(Some(Duration::from_secs(3))),
            Some(Duration::from_secs(3))
        );
        assert_eq!(d.cap(None), None);
    }

    #[test]
    fn a_limited_deadline_counts_down_and_then_fails() {
        let d = Deadline::new(Some(Duration::from_secs(30)));
        let left = d.remaining().expect("limited");
        assert!(left <= Duration::from_secs(30) && left > Duration::from_secs(29));
        assert!(!d.expired());
        assert!(d.check("connect").is_ok());
    }

    #[test]
    fn an_expired_deadline_reports_the_phase_and_the_limit() {
        let d = Deadline::new(Some(Duration::from_nanos(1)));
        std::thread::sleep(Duration::from_millis(2));
        assert!(d.expired());
        assert_eq!(d.remaining(), Some(Duration::ZERO));
        let err = d.check("the TLS handshake").unwrap_err();
        assert!(err.to_string().contains("the TLS handshake"), "{err}");
        assert!(err.to_string().contains("--max-time"), "{err}");
    }

    #[test]
    fn cap_takes_the_shorter_of_the_two_budgets() {
        let d = Deadline::new(Some(Duration::from_millis(50)));
        assert!(d.cap(Some(Duration::from_secs(10))).unwrap() <= Duration::from_millis(50));
        assert_eq!(d.cap(Some(Duration::ZERO)), Some(Duration::ZERO));
    }
}
