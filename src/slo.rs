//! SLO threshold parsing and checking.
//!
//! A spec like `total=500,connect=100,ttfb=200` maps user-facing keys to timing
//! phases; a measured phase above its threshold is a violation, and any
//! violation makes the process exit with [`EXIT_SLO`](crate::error::EXIT_SLO).

use std::fmt;

use crate::error::{Error, Result};
use crate::timing::Timings;

/// Extracts the timing phase (in ms) an SLO key compares against.
type PhaseFn = fn(&Timings) -> i64;

/// Supported SLO keys, the phase each compares against, and how to explain it.
const SLO_KEYS: &[(&str, PhaseFn, &str)] = &[
    ("total", |t| t.total_ms, "the complete request"),
    ("connect", |t| t.connect_ms, "DNS plus TCP connect"),
    ("ttfb", |t| t.starttransfer_ms, "time to first byte"),
    ("dns", |t| t.namelookup_ms, "DNS resolution"),
    (
        "tls",
        |t| t.pretransfer_ms,
        "everything up to the first request byte",
    ),
];

/// A threshold that was exceeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub key: String,
    pub threshold_ms: i64,
    pub actual_ms: i64,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SLO VIOLATION: {} = {}ms (threshold: {}ms)",
            self.key, self.actual_ms, self.threshold_ms
        )
    }
}

/// A parsed `--slo` specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slo {
    /// (key, threshold_ms), preserving the order the user specified.
    thresholds: Vec<(String, i64)>,
}

impl Slo {
    /// Parse `total=500,connect=100` into thresholds.
    ///
    /// Every malformed entry, unknown key or non-positive value is a usage
    /// error naming the offending part.
    pub fn parse(spec: &str) -> Result<Slo> {
        if spec.trim().is_empty() {
            return Err(Error::usage(format!(
                "empty --slo specification, expected key=milliseconds (valid keys: {})",
                valid_keys()
            )));
        }
        let mut thresholds: Vec<(String, i64)> = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(Error::usage(format!(
                    "empty entry in --slo specification \"{spec}\""
                )));
            }
            let (key, value) = part.split_once('=').ok_or_else(|| {
                Error::usage(format!(
                    "invalid --slo entry \"{part}\", expected key=milliseconds"
                ))
            })?;
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            if lookup(&key).is_none() {
                return Err(Error::usage(format!(
                    "unknown SLO key \"{key}\", valid keys: {}",
                    valid_keys()
                )));
            }
            let ms: i64 = value.parse().map_err(|_| {
                Error::usage(format!(
                    "SLO threshold for \"{key}\" must be a whole number of milliseconds, got \"{value}\""
                ))
            })?;
            if ms <= 0 {
                return Err(Error::usage(format!(
                    "SLO threshold for \"{key}\" must be positive, got {ms}"
                )));
            }
            if let Some(existing) = thresholds.iter_mut().find(|(k, _)| *k == key) {
                // Last one wins, mirroring how repeated flags usually behave.
                existing.1 = ms;
            } else {
                thresholds.push((key, ms));
            }
        }
        Ok(Slo { thresholds })
    }

    /// Compare timings against the thresholds, in spec order.
    pub fn check(&self, timings: &Timings) -> Vec<Violation> {
        self.thresholds
            .iter()
            .filter_map(|(key, threshold)| {
                let actual = lookup(key).expect("keys are validated at parse time")(timings);
                (actual > *threshold).then(|| Violation {
                    key: key.clone(),
                    threshold_ms: *threshold,
                    actual_ms: actual,
                })
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.thresholds.is_empty()
    }
}

fn lookup(key: &str) -> Option<PhaseFn> {
    SLO_KEYS
        .iter()
        .find(|(k, ..)| *k == key)
        .map(|(_, f, _)| *f)
}

/// The valid keys, formatted for an error message or help text.
pub fn valid_keys() -> String {
    SLO_KEYS
        .iter()
        .map(|(k, ..)| *k)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The valid keys with their explanations, for the long help text.
pub fn key_help() -> Vec<String> {
    SLO_KEYS
        .iter()
        .map(|(key, _, help)| format!("{key} ({help})"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timings() -> Timings {
        Timings {
            namelookup_ms: 10,
            connect_ms: 40,
            pretransfer_ms: 90,
            starttransfer_ms: 150,
            total_ms: 200,
        }
    }

    #[test]
    fn parses_multiple_thresholds_in_order() {
        let slo = Slo::parse("total=500, connect=100,ttfb=200").unwrap();
        assert_eq!(
            slo.thresholds,
            [
                ("total".to_string(), 500),
                ("connect".to_string(), 100),
                ("ttfb".to_string(), 200)
            ]
        );
    }

    #[test]
    fn keys_are_case_insensitive_and_last_repeat_wins() {
        let slo = Slo::parse("TOTAL=500,total=100").unwrap();
        assert_eq!(slo.thresholds, [("total".to_string(), 100)]);
    }

    #[test]
    fn every_documented_key_resolves_to_its_phase() {
        let t = timings();
        for (key, expected) in [
            ("total", 200),
            ("connect", 40),
            ("ttfb", 150),
            ("dns", 10),
            ("tls", 90),
        ] {
            let violations = Slo::parse(&format!("{key}=1")).unwrap().check(&t);
            assert_eq!(violations.len(), 1, "{key}");
            assert_eq!(violations[0].actual_ms, expected, "{key}");
        }
    }

    #[test]
    fn violations_report_the_offending_phase_in_spec_order() {
        let slo = Slo::parse("dns=1000,total=100,ttfb=10").unwrap();
        let violations = slo.check(&timings());
        assert_eq!(
            violations,
            [
                Violation {
                    key: "total".into(),
                    threshold_ms: 100,
                    actual_ms: 200
                },
                Violation {
                    key: "ttfb".into(),
                    threshold_ms: 10,
                    actual_ms: 150
                },
            ]
        );
        assert_eq!(
            violations[0].to_string(),
            "SLO VIOLATION: total = 200ms (threshold: 100ms)"
        );
    }

    #[test]
    fn a_threshold_met_exactly_is_not_a_violation() {
        assert!(Slo::parse("total=200")
            .unwrap()
            .check(&timings())
            .is_empty());
        assert_eq!(Slo::parse("total=199").unwrap().check(&timings()).len(), 1);
    }

    #[test]
    fn malformed_specs_are_usage_errors_that_name_the_problem() {
        for (spec, expected) in [
            ("", "empty --slo"),
            ("   ", "empty --slo"),
            ("total", "expected key=milliseconds"),
            ("bogus=10", "unknown SLO key"),
            ("total=abc", "whole number of milliseconds"),
            ("total=0", "must be positive"),
            ("total=-5", "must be positive"),
            ("total=1,", "empty entry"),
        ] {
            let err = Slo::parse(spec).unwrap_err();
            assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{spec}");
            assert!(
                err.to_string().contains(expected),
                "spec {spec:?} produced {err}"
            );
        }
    }

    #[test]
    fn the_error_message_lists_the_valid_keys() {
        let err = Slo::parse("bogus=10").unwrap_err().to_string();
        for (key, ..) in SLO_KEYS {
            assert!(err.contains(key), "{err} is missing {key}");
        }
        assert_eq!(key_help().len(), SLO_KEYS.len());
    }
}
