//! Timing phases and derived ranges, all expressed in integer milliseconds to
//! match the original tool's output semantics.

use std::time::Duration;

fn to_ms(d: Duration) -> i64 {
    (d.as_secs_f64() * 1000.0).round() as i64
}

/// Cumulative timing milestones, each measured from the start of the request.
/// Field names mirror curl's `time_*` getinfo values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub namelookup_ms: i64,
    pub connect_ms: i64,
    pub pretransfer_ms: i64,
    pub starttransfer_ms: i64,
    pub total_ms: i64,
}

/// The five visual segments shown in the pretty output, derived from the
/// cumulative [`Timings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranges {
    pub dns: i64,
    pub connection: i64,
    pub ssl: i64,
    pub server: i64,
    pub transfer: i64,
}

impl Timings {
    pub fn from_durations(
        namelookup: Duration,
        connect: Duration,
        pretransfer: Duration,
        starttransfer: Duration,
        total: Duration,
    ) -> Self {
        Timings {
            namelookup_ms: to_ms(namelookup),
            connect_ms: to_ms(connect),
            pretransfer_ms: to_ms(pretransfer),
            starttransfer_ms: to_ms(starttransfer),
            total_ms: to_ms(total),
        }
    }

    /// Split the cumulative milestones into per-phase durations. Differences are
    /// clamped at zero so jitter can never produce a negative segment.
    pub fn ranges(&self) -> Ranges {
        let nonneg = |x: i64| x.max(0);
        Ranges {
            dns: nonneg(self.namelookup_ms),
            connection: nonneg(self.connect_ms - self.namelookup_ms),
            ssl: nonneg(self.pretransfer_ms - self.connect_ms),
            server: nonneg(self.starttransfer_ms - self.pretransfer_ms),
            transfer: nonneg(self.total_ms - self.starttransfer_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_differences_of_milestones() {
        let t = Timings {
            namelookup_ms: 5,
            connect_ms: 15,
            pretransfer_ms: 30,
            starttransfer_ms: 80,
            total_ms: 100,
        };
        let r = t.ranges();
        assert_eq!(r.dns, 5);
        assert_eq!(r.connection, 10);
        assert_eq!(r.ssl, 15);
        assert_eq!(r.server, 50);
        assert_eq!(r.transfer, 20);
    }

    #[test]
    fn ranges_never_go_negative() {
        let t = Timings {
            namelookup_ms: 10,
            connect_ms: 5,
            pretransfer_ms: 5,
            starttransfer_ms: 5,
            total_ms: 5,
        };
        let r = t.ranges();
        assert_eq!(r.connection, 0);
    }
}
