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

    /// Element-wise arithmetic mean of several samples, rounded to whole ms.
    /// Used by `--count` to report averaged timings across repeated runs.
    pub fn mean(samples: &[Timings]) -> Timings {
        let n = samples.len().max(1) as f64;
        let avg = |get: fn(&Timings) -> i64| {
            (samples.iter().map(|t| get(t) as f64).sum::<f64>() / n).round() as i64
        };
        Timings {
            namelookup_ms: avg(|t| t.namelookup_ms),
            connect_ms: avg(|t| t.connect_ms),
            pretransfer_ms: avg(|t| t.pretransfer_ms),
            starttransfer_ms: avg(|t| t.starttransfer_ms),
            total_ms: avg(|t| t.total_ms),
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

/// Spread of the `total` timing across repeated runs (`--count`), summarising
/// how stable the measurements were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotalStats {
    pub runs: usize,
    pub min_ms: i64,
    pub mean_ms: i64,
    pub max_ms: i64,
}

impl TotalStats {
    pub fn from_samples(samples: &[Timings]) -> TotalStats {
        let totals: Vec<i64> = samples.iter().map(|t| t.total_ms).collect();
        let runs = totals.len().max(1);
        TotalStats {
            runs,
            min_ms: totals.iter().copied().min().unwrap_or(0),
            max_ms: totals.iter().copied().max().unwrap_or(0),
            mean_ms: (totals.iter().sum::<i64>() as f64 / runs as f64).round() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(total: i64) -> Timings {
        Timings {
            namelookup_ms: 5,
            connect_ms: 15,
            pretransfer_ms: 30,
            starttransfer_ms: 80,
            total_ms: total,
        }
    }

    #[test]
    fn mean_averages_each_milestone() {
        let samples = [
            Timings {
                namelookup_ms: 10,
                connect_ms: 20,
                pretransfer_ms: 30,
                starttransfer_ms: 40,
                total_ms: 50,
            },
            Timings {
                namelookup_ms: 20,
                connect_ms: 40,
                pretransfer_ms: 60,
                starttransfer_ms: 80,
                total_ms: 100,
            },
        ];
        let m = Timings::mean(&samples);
        assert_eq!(m.namelookup_ms, 15);
        assert_eq!(m.total_ms, 75);
    }

    #[test]
    fn total_stats_capture_spread() {
        let s = TotalStats::from_samples(&[sample(100), sample(200), sample(300)]);
        assert_eq!(s.runs, 3);
        assert_eq!(s.min_ms, 100);
        assert_eq!(s.mean_ms, 200);
        assert_eq!(s.max_ms, 300);
    }

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
