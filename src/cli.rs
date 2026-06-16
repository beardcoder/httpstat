//! Command-line argument and environment-variable handling.

use clap::Parser;

pub const USER_AGENT_DEFAULT: &str = concat!("httpstat-rs/", env!("CARGO_PKG_VERSION"));

const LONG_ABOUT: &str = "\
httpstat visualizes HTTP request timings (DNS, TCP, TLS, server, transfer) in a
clear terminal layout, with structured JSON output and SLO threshold checks.

This is a native Rust port: the HTTP request is performed in-process (no curl
binary), so only the documented options below are supported — not arbitrary
curl flags.

Environment variables:
  HTTPSTAT_SHOW_BODY    Show response body (truncated to 1024 bytes). Default false.
  HTTPSTAT_SHOW_IP      Show remote/local IP and port. Default true.
  HTTPSTAT_SHOW_SPEED   Show download/upload speed. Default false.
  HTTPSTAT_SAVE_BODY    Store body in a temp file. Default true.
  HTTPSTAT_METRICS_ONLY Equivalent to --format json. Default false.
  HTTPSTAT_DEBUG        Print resolved options to stderr. Default false.
  NO_COLOR              Disable colored output (https://no-color.org).";

#[derive(Parser, Debug)]
#[command(
    name = "httpstat",
    version,
    about = "curl statistics made simple — native Rust",
    long_about = LONG_ABOUT,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// URL to request, with or without an http(s):// prefix.
    pub url: Option<String>,

    /// Output format: pretty, json, or jsonl.
    #[arg(
        short = 'f',
        long = "format",
        default_value = "pretty",
        value_name = "FORMAT"
    )]
    pub format: String,

    /// SLO thresholds as key=value pairs, e.g. total=500,connect=100.
    /// Valid keys: total, connect, ttfb, dns, tls. Exits with code 4 on violation.
    #[arg(long = "slo", value_name = "SPEC")]
    pub slo: Option<String>,

    /// Save the structured JSON result to a file path.
    #[arg(long = "save", value_name = "PATH")]
    pub save: Option<String>,

    /// HTTP request method (defaults to GET, or POST when --data is given).
    #[arg(short = 'X', long = "request", value_name = "METHOD")]
    pub method: Option<String>,

    /// Extra request header "Name: Value" (repeatable).
    #[arg(short = 'H', long = "header", value_name = "HEADER")]
    pub headers: Vec<String>,

    /// Request body data.
    #[arg(short = 'd', long = "data", value_name = "DATA")]
    pub data: Option<String>,

    /// Follow HTTP redirects.
    #[arg(short = 'L', long = "location")]
    pub follow: bool,

    /// Skip TLS certificate verification.
    #[arg(short = 'k', long = "insecure")]
    pub insecure: bool,

    /// User-Agent header value.
    #[arg(short = 'A', long = "user-agent", default_value = USER_AGENT_DEFAULT, value_name = "UA")]
    pub user_agent: String,

    /// Maximum time allowed for the TCP connection, in seconds.
    #[arg(long = "connect-timeout", value_name = "SECONDS")]
    pub connect_timeout: Option<f64>,

    /// Maximum total time allowed for the transfer, in seconds.
    #[arg(long = "max-time", value_name = "SECONDS")]
    pub max_time: Option<f64>,
}

/// Resolve the effective HTTP method: explicit `-X`, else POST when a body is
/// present (mirroring curl), else GET.
pub fn effective_method(method: &Option<String>, has_body: bool) -> String {
    match method {
        Some(m) => m.clone(),
        None if has_body => "POST".to_string(),
        None => "GET".to_string(),
    }
}

/// Parse a "Name: Value" header string. Tolerates the space-less "Name:Value".
pub fn parse_header(raw: &str) -> Result<(String, String), String> {
    let (k, v) = raw
        .split_once(':')
        .ok_or_else(|| format!("invalid header \"{raw}\", expected \"Name: Value\""))?;
    Ok((k.trim().to_string(), v.trim().to_string()))
}

const TRUTHY: &[&str] = &["1", "true", "yes", "on"];
const FALSY: &[&str] = &["0", "false", "no", "off"];

/// Strict boolean parsing matching the original `parse_bool`.
pub fn parse_bool(value: &str) -> Result<bool, String> {
    let v = value.trim().to_ascii_lowercase();
    if TRUTHY.contains(&v.as_str()) {
        Ok(true)
    } else if FALSY.contains(&v.as_str()) {
        Ok(false)
    } else {
        Err(format!("invalid boolean value: {value:?}"))
    }
}

/// Read a `HTTPSTAT_*` boolean environment variable with a default.
pub fn env_bool(key: &str, default: bool) -> Result<bool, String> {
    match std::env::var(key) {
        Ok(v) => parse_bool(&v).map_err(|e| format!("{key}: {e}")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_accepts_known_truthy_and_falsy() {
        for v in ["1", "true", "YES", " On "] {
            assert!(parse_bool(v).unwrap());
        }
        for v in ["0", "false", "NO", "off"] {
            assert!(!parse_bool(v).unwrap());
        }
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn method_defaults_to_post_with_body() {
        assert_eq!(effective_method(&None, false), "GET");
        assert_eq!(effective_method(&None, true), "POST");
        assert_eq!(effective_method(&Some("PUT".into()), true), "PUT");
    }

    #[test]
    fn header_parsing_trims() {
        assert_eq!(
            parse_header("X-Test: 1").unwrap(),
            ("X-Test".into(), "1".into())
        );
        assert_eq!(
            parse_header("X-Test:1").unwrap(),
            ("X-Test".into(), "1".into())
        );
        assert!(parse_header("nope").is_err());
    }
}
