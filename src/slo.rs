//! SLO threshold parsing and checking.
//!
//! A spec like `total=500,connect=100,ttfb=200` maps user-facing keys to timing
//! phases and yields violations when a measured phase exceeds its threshold (ms).

use crate::timing::Timings;

/// Extracts the timing phase (in ms) an SLO key compares against.
type PhaseFn = fn(&Timings) -> i64;

/// Supported SLO keys and the timing phase (in ms) each one compares against.
const SLO_KEYS: &[(&str, PhaseFn)] = &[
    ("total", |t| t.total_ms),
    ("connect", |t| t.connect_ms),
    ("ttfb", |t| t.starttransfer_ms),
    ("dns", |t| t.namelookup_ms),
    ("tls", |t| t.pretransfer_ms),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub key: String,
    pub threshold_ms: i64,
    pub actual_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slo {
    /// (key, threshold_ms), preserving the order the user specified.
    thresholds: Vec<(String, i64)>,
}

fn lookup(key: &str) -> Option<PhaseFn> {
    SLO_KEYS.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)
}

fn valid_keys() -> String {
    SLO_KEYS
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Slo {
    /// Parse `total=500,connect=100` into thresholds. Returns a human-readable
    /// error string on any malformed entry, unknown key, or non-positive value.
    pub fn parse(spec: &str) -> Result<Slo, String> {
        let mut thresholds = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err("empty SLO spec".to_string());
            }
            let (key, val) = part
                .split_once('=')
                .ok_or_else(|| format!("invalid SLO spec \"{part}\", expected key=value"))?;
            let key = key.trim();
            let val = val.trim();
            if lookup(key).is_none() {
                return Err(format!(
                    "unknown SLO key \"{key}\", valid keys: {}",
                    valid_keys()
                ));
            }
            let ms: i64 = val.parse().map_err(|_| {
                format!("SLO value for \"{key}\" must be a positive integer, got \"{val}\"")
            })?;
            if ms <= 0 {
                return Err(format!(
                    "SLO value for \"{key}\" must be positive, got {ms}"
                ));
            }
            thresholds.push((key.to_string(), ms));
        }
        Ok(Slo { thresholds })
    }

    /// Compare timings against the thresholds and return any violations,
    /// preserving spec order.
    pub fn check(&self, timings: &Timings) -> Vec<Violation> {
        self.thresholds
            .iter()
            .filter_map(|(key, threshold)| {
                let actual = lookup(key).expect("key validated at parse time")(timings);
                (actual > *threshold).then(|| Violation {
                    key: key.clone(),
                    threshold_ms: *threshold,
                    actual_ms: actual,
                })
            })
            .collect()
    }
}
