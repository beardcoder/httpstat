//! Output rendering: the structured JSON schema and the pretty terminal view.

pub mod json;
pub mod pretty;

use std::fmt;

use crate::error::{Error, Result};
use crate::http::HttpResult;
use crate::slo::Violation;
use crate::timing::TotalStats;

/// Selected output format (`--format` / `-f`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// The colored terminal visualization.
    #[default]
    Pretty,
    /// Indented JSON.
    Json,
    /// Single-line JSON, one record per line.
    Jsonl,
}

impl Format {
    pub const ALL: &'static [&'static str] = &["pretty", "json", "jsonl"];

    pub fn parse(value: &str) -> Result<Format> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pretty" => Ok(Format::Pretty),
            "json" => Ok(Format::Json),
            "jsonl" => Ok(Format::Jsonl),
            other => Err(Error::usage(format!(
                "invalid format \"{other}\", must be one of: {}",
                Format::ALL.join(", ")
            ))),
        }
    }

    /// Whether this format serializes as multi-line JSON.
    pub fn is_indented_json(&self) -> bool {
        matches!(self, Format::Json)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Pretty => "pretty",
            Format::Json => "json",
            Format::Jsonl => "jsonl",
        })
    }
}

/// Everything the renderers need, assembled once by the application layer.
///
/// Passing one struct rather than ten positional arguments is what keeps the
/// two renderers honest about showing the same numbers.
#[derive(Debug)]
pub struct Report<'a> {
    pub result: &'a HttpResult,
    /// `GET https://example.com`, shown as the first pretty line.
    pub request_line: String,
    pub download_kbs: f64,
    pub upload_kbs: f64,
    /// Violations found, empty when everything passed.
    pub violations: Vec<Violation>,
    /// Whether `--slo` was given at all — an empty violation list means "passed"
    /// only when it was.
    pub slo_requested: bool,
    /// How many times the request was issued (`--count`).
    pub runs: u32,
    /// Distribution of the total time, present when `runs > 1`.
    pub stats: Option<TotalStats>,
    pub exit_code: u8,
}

impl Report<'_> {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

/// Convert bytes-over-seconds into KiB/s rounded to one decimal place, matching
/// the original output. Returns `0.0` when no measurable transfer occurred.
pub fn kbs(bytes: usize, secs: f64) -> f64 {
    if !secs.is_finite() || secs <= 0.0 || bytes == 0 {
        return 0.0;
    }
    let kbs = bytes as f64 / secs / 1024.0;
    (kbs * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_case_insensitively_with_surrounding_space() {
        assert_eq!(Format::parse("pretty").unwrap(), Format::Pretty);
        assert_eq!(Format::parse(" JSON ").unwrap(), Format::Json);
        assert_eq!(Format::parse("jsonl").unwrap(), Format::Jsonl);
        assert_eq!(Format::default(), Format::Pretty);
    }

    #[test]
    fn an_unknown_format_is_a_usage_error_listing_the_valid_ones() {
        let err = Format::parse("xml").unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        let message = err.to_string();
        assert!(message.contains("invalid format \"xml\""), "{message}");
        for name in Format::ALL {
            assert!(message.contains(name), "{message} is missing {name}");
        }
    }

    #[test]
    fn formats_round_trip_through_their_display_name() {
        for name in Format::ALL {
            assert_eq!(Format::parse(name).unwrap().to_string(), *name);
        }
        assert!(Format::Json.is_indented_json());
        assert!(!Format::Jsonl.is_indented_json());
    }

    #[test]
    fn transfer_rate_is_kibibytes_per_second_to_one_decimal() {
        assert_eq!(kbs(1024, 1.0), 1.0);
        assert_eq!(kbs(2048, 0.5), 4.0);
        assert_eq!(kbs(1000, 1.0), 1.0);
        assert_eq!(kbs(5000, 2.0), 2.4);
    }

    #[test]
    fn transfer_rate_is_zero_when_it_cannot_be_measured() {
        assert_eq!(kbs(0, 1.0), 0.0);
        assert_eq!(kbs(1024, 0.0), 0.0);
        assert_eq!(kbs(1024, -1.0), 0.0);
        assert_eq!(kbs(1024, f64::NAN), 0.0);
    }
}
