//! Output rendering: the structured JSON schema and the pretty terminal view.

pub mod json;
pub mod pretty;

/// Selected output format (`--format` / `-f`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Pretty,
    Json,
    Jsonl,
}

impl Format {
    pub fn parse(value: &str) -> Result<Format, String> {
        match value {
            "pretty" => Ok(Format::Pretty),
            "json" => Ok(Format::Json),
            "jsonl" => Ok(Format::Jsonl),
            other => Err(format!(
                "invalid format \"{other}\", must be pretty, json, or jsonl"
            )),
        }
    }
}

/// Convert bytes-over-seconds into KiB/s rounded to one decimal place, matching
/// the original output. Returns `0.0` when no measurable transfer occurred.
pub fn kbs(bytes: usize, secs: f64) -> f64 {
    if secs <= 0.0 || bytes == 0 {
        return 0.0;
    }
    let kbs = bytes as f64 / secs / 1024.0;
    (kbs * 10.0).round() / 10.0
}
