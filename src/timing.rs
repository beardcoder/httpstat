//! Timing phases and derived ranges, all expressed in integer milliseconds to
//! match the original tool's output semantics.

use std::time::Duration;

fn to_ms(d: Duration) -> i64 {
    (d.as_secs_f64() * 1000.0).round() as i64
}

/// Cumulative timing milestones, each measured from the start of the request.
/// Field names mirror curl's `time_*` getinfo values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
        if samples.is_empty() {
            return Timings::default();
        }
        let n = samples.len() as f64;
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

/// Distribution of the `total` timing across repeated runs (`--count`).
///
/// A mean on its own hides a bimodal connection; the median and the 95th
/// percentile are what tell you whether a slow mean is everyone's problem or a
/// long tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TotalStats {
    pub runs: usize,
    pub min_ms: i64,
    pub p50_ms: i64,
    pub mean_ms: i64,
    pub p95_ms: i64,
    pub max_ms: i64,
}

impl TotalStats {
    /// Summarize the total time of each sample. Returns `None` for no samples.
    pub fn from_samples(samples: &[Timings]) -> Option<TotalStats> {
        if samples.is_empty() {
            return None;
        }
        let mut totals: Vec<i64> = samples.iter().map(|t| t.total_ms).collect();
        totals.sort_unstable();
        let runs = totals.len();
        Some(TotalStats {
            runs,
            min_ms: totals[0],
            p50_ms: percentile(&totals, 50.0),
            mean_ms: (totals.iter().sum::<i64>() as f64 / runs as f64).round() as i64,
            p95_ms: percentile(&totals, 95.0),
            max_ms: totals[runs - 1],
        })
    }
}

/// Nearest-rank percentile of an ascending slice: the smallest value at or above
/// which `pct` percent of the samples fall. Simple, exact, and never
/// interpolates a number that was not measured.
fn percentile(sorted: &[i64], pct: f64) -> i64 {
    debug_assert!(!sorted.is_empty());
    let rank = (pct / 100.0 * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
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
    fn durations_are_rounded_to_whole_milliseconds() {
        let t = Timings::from_durations(
            Duration::from_micros(1400),
            Duration::from_micros(1600),
            Duration::from_millis(3),
            Duration::from_millis(4),
            Duration::from_millis(5),
        );
        assert_eq!(t.namelookup_ms, 1);
        assert_eq!(t.connect_ms, 2);
        assert_eq!(t.total_ms, 5);
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
        assert_eq!(m.connect_ms, 30);
        assert_eq!(m.total_ms, 75);
        assert_eq!(Timings::mean(&[]), Timings::default());
    }

    #[test]
    fn total_stats_capture_the_distribution() {
        let s = TotalStats::from_samples(&[sample(300), sample(100), sample(200)]).unwrap();
        assert_eq!(s.runs, 3);
        assert_eq!(s.min_ms, 100);
        assert_eq!(s.p50_ms, 200);
        assert_eq!(s.mean_ms, 200);
        assert_eq!(s.max_ms, 300);
        assert_eq!(TotalStats::from_samples(&[]), None);
    }

    #[test]
    fn a_single_sample_is_its_own_distribution() {
        let s = TotalStats::from_samples(&[sample(42)]).unwrap();
        assert_eq!(
            (s.min_ms, s.p50_ms, s.mean_ms, s.p95_ms, s.max_ms),
            (42, 42, 42, 42, 42)
        );
    }

    #[test]
    fn the_p95_follows_the_slow_tail_rather_than_the_mean() {
        let samples: Vec<Timings> = (1..=100).map(|i| sample(i * 10)).collect();
        let s = TotalStats::from_samples(&samples).unwrap();
        assert_eq!(s.p50_ms, 500);
        assert_eq!(s.p95_ms, 950);
        assert_eq!(s.max_ms, 1000);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let sorted = [10, 20, 30, 40];
        assert_eq!(percentile(&sorted, 0.0), 10);
        assert_eq!(percentile(&sorted, 25.0), 10);
        assert_eq!(percentile(&sorted, 50.0), 20);
        assert_eq!(percentile(&sorted, 100.0), 40);
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
        assert_eq!(r.transfer, 0);
    }
}
