//! Command-line arguments and `HTTPSTAT_*` environment variables.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::error::{Error, Result};
use crate::http::headers::Headers;
use crate::http::request::{self, RequestOptions};
use crate::output::Format;

/// Default `User-Agent`, e.g. `httpstat-rs/2.3.0`.
pub const USER_AGENT_DEFAULT: &str = crate::USER_AGENT;

const LONG_ABOUT: &str = "\
httpstat measures where the time in an HTTP request actually goes, and shows it
as a labelled timeline: DNS lookup, TCP connection, TLS handshake, server
processing and content transfer.

The request is issued in-process by a small HTTP/1.1 client — there is no curl
binary and no OpenSSL — so each phase is timed at the point it happens rather
than inferred afterwards. Only the options documented below are supported, not
arbitrary curl flags.

Beyond the terminal view it can emit the same measurements as JSON for
dashboards and log pipelines (--format), average a run of requests to see past
jitter (--count), and fail a build or an alert when a phase breaches a threshold
(--slo).

Exit codes:
  0  the request completed, and any --slo thresholds held
  1  the request failed (DNS, connection, TLS, protocol, timeout)
  2  the command line or environment was invalid
  4  the request completed but breached an --slo threshold

Environment variables:
  HTTPSTAT_SHOW_BODY    Show the response body (first 1024 bytes). Default false.
  HTTPSTAT_SHOW_IP      Show remote/local IP and port. Default true.
  HTTPSTAT_SHOW_SPEED   Show download/upload speed. Default false.
  HTTPSTAT_SAVE_BODY    Store the body in a temporary file. Default true.
  HTTPSTAT_METRICS_ONLY Equivalent to --format json. Default false.
  HTTPSTAT_DEBUG        Print the resolved options to stderr. Default false.
  SSL_CERT_FILE         PEM bundle to verify TLS against, as --cacert does.
  NO_COLOR              Disable colored output (https://no-color.org).

Booleans accept 1/true/yes/on and 0/false/no/off.";

/// Parsed command line.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "httpstat",
    version,
    about = "Visualize where the time goes in an HTTP request",
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

    /// Repeat the request N times and report the averaged timings.
    #[arg(
        short = 'n',
        long = "count",
        default_value_t = 1,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub count: u32,

    /// SLO thresholds in milliseconds, e.g. total=500,connect=100.
    /// Valid keys: total, connect, ttfb, dns, tls. Exits with code 4 on violation.
    #[arg(long = "slo", value_name = "SPEC")]
    pub slo: Option<String>,

    /// Write the structured JSON result to a file, whatever --format is set to.
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

    /// Maximum number of redirects to follow.
    #[arg(
        long = "max-redirects",
        default_value_t = 10,
        value_name = "N",
        requires = "follow"
    )]
    pub max_redirects: u32,

    /// Skip TLS certificate verification. Only for hosts you already trust.
    #[arg(short = 'k', long = "insecure", conflicts_with = "cacert")]
    pub insecure: bool,

    /// Verify TLS against this PEM bundle instead of the built-in roots.
    /// Defaults to $SSL_CERT_FILE when that is set.
    #[arg(long = "cacert", value_name = "PATH")]
    pub cacert: Option<PathBuf>,

    /// User-Agent header value.
    #[arg(short = 'A', long = "user-agent", default_value = USER_AGENT_DEFAULT, value_name = "UA")]
    pub user_agent: String,

    /// Maximum time allowed for the TCP connection, in seconds.
    #[arg(long = "connect-timeout", value_name = "SECONDS")]
    pub connect_timeout: Option<f64>,

    /// Maximum total time allowed for the request, in seconds, redirects included.
    #[arg(long = "max-time", value_name = "SECONDS")]
    pub max_time: Option<f64>,
}

impl Cli {
    /// Turn the command line into request options, validating as it goes.
    pub fn request_options(&self) -> Result<RequestOptions> {
        let body = self.data.clone().map(String::into_bytes);
        let mut headers = Headers::new();
        for raw in &self.headers {
            let (name, value) = parse_header(raw)?;
            headers.push(name, value);
        }
        let opts = RequestOptions {
            method: effective_method(self.method.as_deref(), body.is_some()),
            headers,
            body,
            follow_redirects: self.follow,
            max_redirects: self.max_redirects as usize,
            insecure: self.insecure,
            ca_file: self.ca_bundle(),
            user_agent: self.user_agent.clone(),
            connect_timeout: duration("--connect-timeout", self.connect_timeout)?,
            max_time: duration("--max-time", self.max_time)?,
        };
        opts.validate()?;
        Ok(opts)
    }

    pub fn format(&self) -> Result<Format> {
        Format::parse(&self.format)
    }

    /// The CA bundle to verify against: `--cacert`, else `$SSL_CERT_FILE`
    /// (the convention curl and OpenSSL already follow), else the built-in roots.
    fn ca_bundle(&self) -> Option<PathBuf> {
        self.cacert.clone().or_else(|| {
            std::env::var_os("SSL_CERT_FILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
    }
}

/// Convert a seconds flag into a `Duration`, rejecting values a clock cannot hold.
fn duration(flag: &str, seconds: Option<f64>) -> Result<Option<Duration>> {
    let Some(seconds) = seconds else {
        return Ok(None);
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(Error::usage(format!(
            "{flag} must be a positive number of seconds, got {seconds}"
        )));
    }
    Duration::try_from_secs_f64(seconds)
        .map(Some)
        .map_err(|_| Error::usage(format!("{flag} value {seconds} is out of range")))
}

/// Resolve the effective HTTP method: explicit `-X`, else POST when a body is
/// present (mirroring curl), else GET.
pub fn effective_method(method: Option<&str>, has_body: bool) -> String {
    match method {
        Some(m) => m.to_string(),
        None if has_body => "POST".to_string(),
        None => "GET".to_string(),
    }
}

/// Parse a `Name: Value` header argument. Tolerates the space-less `Name:Value`.
pub fn parse_header(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw.split_once(':').ok_or_else(|| {
        Error::usage(format!(
            "invalid header \"{raw}\", expected \"Name: Value\""
        ))
    })?;
    let (name, value) = (name.trim(), value.trim());
    request::validate_header(name, value)?;
    Ok((name.to_string(), value.to_string()))
}

const TRUTHY: &[&str] = &["1", "true", "yes", "on"];
const FALSY: &[&str] = &["0", "false", "no", "off"];

/// Parse a boolean environment value, accepting the documented spellings only.
pub fn parse_bool(value: &str) -> Result<bool> {
    let normalized = value.trim().to_ascii_lowercase();
    if TRUTHY.contains(&normalized.as_str()) {
        Ok(true)
    } else if FALSY.contains(&normalized.as_str()) {
        Ok(false)
    } else {
        Err(Error::usage(format!(
            "invalid boolean value {value:?}, expected one of: {}, {}",
            TRUTHY.join("/"),
            FALSY.join("/")
        )))
    }
}

/// The `HTTPSTAT_*` toggles, resolved once at start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvOptions {
    pub show_body: bool,
    pub show_ip: bool,
    pub show_speed: bool,
    pub save_body: bool,
    pub metrics_only: bool,
    pub debug: bool,
}

impl Default for EnvOptions {
    fn default() -> Self {
        EnvOptions {
            show_body: false,
            show_ip: true,
            show_speed: false,
            save_body: true,
            metrics_only: false,
            debug: false,
        }
    }
}

impl EnvOptions {
    /// Read the toggles from the process environment.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Read the toggles from an arbitrary source, which is what makes them
    /// testable without mutating the environment of the whole test binary.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let defaults = EnvOptions::default();
        let read = |key: &str, default: bool| -> Result<bool> {
            match lookup(key) {
                Some(raw) => parse_bool(&raw).map_err(|e| Error::usage(format!("{key}: {e}"))),
                None => Ok(default),
            }
        };
        Ok(EnvOptions {
            show_body: read("HTTPSTAT_SHOW_BODY", defaults.show_body)?,
            show_ip: read("HTTPSTAT_SHOW_IP", defaults.show_ip)?,
            show_speed: read("HTTPSTAT_SHOW_SPEED", defaults.show_speed)?,
            save_body: read("HTTPSTAT_SAVE_BODY", defaults.save_body)?,
            metrics_only: read("HTTPSTAT_METRICS_ONLY", defaults.metrics_only)?,
            debug: read("HTTPSTAT_DEBUG", defaults.debug)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;
    use clap::CommandFactory;
    use std::path::Path;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("httpstat").chain(args.iter().copied()))
            .expect("arguments should parse")
    }

    #[test]
    fn the_command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_produce_a_plain_get() {
        let opts = cli(&["example.com"]).request_options().unwrap();
        assert_eq!(opts.method, "GET");
        assert!(opts.body.is_none());
        assert!(!opts.follow_redirects);
        assert!(!opts.insecure);
        assert_eq!(opts.max_redirects, 10);
        assert_eq!(opts.connect_timeout, None);
        assert_eq!(opts.max_time, None);
        assert!(opts.user_agent.starts_with("httpstat-rs/"));
    }

    #[test]
    fn a_data_argument_implies_post_unless_a_method_is_given() {
        let opts = cli(&["example.com", "-d", "hello"])
            .request_options()
            .unwrap();
        assert_eq!(opts.method, "POST");
        assert_eq!(opts.body.as_deref(), Some(&b"hello"[..]));

        let opts = cli(&["example.com", "-d", "hello", "-X", "PUT"])
            .request_options()
            .unwrap();
        assert_eq!(opts.method, "PUT");
        assert_eq!(effective_method(None, false), "GET");
    }

    #[test]
    fn headers_are_parsed_trimmed_and_repeatable() {
        let opts = cli(&["example.com", "-H", "X-A: 1", "-H", "X-B:2", "-H", "X-A: 3"])
            .request_options()
            .unwrap();
        assert_eq!(opts.headers.get_all("x-a").collect::<Vec<_>>(), ["1", "3"]);
        assert_eq!(opts.headers.get("x-b"), Some("2"));
    }

    #[test]
    fn malformed_and_dangerous_headers_are_usage_errors() {
        for raw in ["nope", "Bad Name: 1", "X-Test: a\r\nX-Evil: 1"] {
            let err = parse_header(raw).unwrap_err();
            assert_eq!(err.exit_code(), EXIT_USAGE, "{raw}");
        }
        assert_eq!(
            parse_header("X-Test: 1").unwrap(),
            ("X-Test".into(), "1".into())
        );
        assert_eq!(
            parse_header("X-Test:1").unwrap(),
            ("X-Test".into(), "1".into())
        );
    }

    #[test]
    fn timeouts_are_converted_to_durations_and_validated() {
        let opts = cli(&[
            "example.com",
            "--connect-timeout",
            "1.5",
            "--max-time",
            "10",
        ])
        .request_options()
        .unwrap();
        assert_eq!(opts.connect_timeout, Some(Duration::from_millis(1500)));
        assert_eq!(opts.max_time, Some(Duration::from_secs(10)));

        for bad in [
            "--max-time=0",
            "--max-time=-1",
            "--max-time=nan",
            "--max-time=inf",
        ] {
            let err = cli(&["example.com", bad]).request_options().unwrap_err();
            assert_eq!(err.exit_code(), EXIT_USAGE, "{bad}");
        }
        // A value clap itself cannot read never reaches us.
        assert!(Cli::try_parse_from(["httpstat", "example.com", "--max-time=x"]).is_err());
    }

    #[test]
    fn the_format_flag_is_parsed_into_a_format() {
        assert_eq!(cli(&["example.com"]).format().unwrap(), Format::Pretty);
        assert_eq!(
            cli(&["example.com", "-f", "jsonl"]).format().unwrap(),
            Format::Jsonl
        );
        assert!(cli(&["example.com", "-f", "xml"]).format().is_err());
    }

    #[test]
    fn count_must_be_at_least_one() {
        assert!(Cli::try_parse_from(["httpstat", "example.com", "-n", "0"]).is_err());
        assert_eq!(cli(&["example.com", "-n", "5"]).count, 5);
    }

    #[test]
    fn a_ca_bundle_can_be_given_explicitly() {
        let opts = cli(&["example.com", "--cacert", "/etc/ssl/private.pem"])
            .request_options()
            .unwrap();
        assert_eq!(
            opts.ca_file.as_deref(),
            Some(Path::new("/etc/ssl/private.pem"))
        );
        // Skipping verification and pinning a bundle are contradictory asks.
        assert!(
            Cli::try_parse_from(["httpstat", "example.com", "-k", "--cacert", "/x.pem"]).is_err()
        );
    }

    #[test]
    fn max_redirects_requires_follow() {
        assert!(Cli::try_parse_from(["httpstat", "example.com", "--max-redirects", "2"]).is_err());
        let opts = cli(&["example.com", "-L", "--max-redirects", "2"])
            .request_options()
            .unwrap();
        assert_eq!(opts.max_redirects, 2);
    }

    #[test]
    fn booleans_accept_the_documented_spellings_only() {
        for value in ["1", "true", "YES", " On "] {
            assert!(parse_bool(value).unwrap(), "{value}");
        }
        for value in ["0", "false", "NO", "off"] {
            assert!(!parse_bool(value).unwrap(), "{value}");
        }
        let err = parse_bool("maybe").unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().contains("invalid boolean value"), "{err}");
    }

    #[test]
    fn env_options_fall_back_to_their_defaults() {
        let options = EnvOptions::from_lookup(|_| None).unwrap();
        assert_eq!(options, EnvOptions::default());
        assert!(options.show_ip && options.save_body);
        assert!(!options.show_body && !options.show_speed && !options.debug);
    }

    #[test]
    fn env_options_read_every_documented_toggle() {
        fn set(key: &'static str) -> impl Fn(&str) -> Option<String> {
            move |k: &str| (k == key).then(|| "1".to_string())
        }
        assert!(
            EnvOptions::from_lookup(set("HTTPSTAT_SHOW_BODY"))
                .unwrap()
                .show_body
        );
        assert!(
            EnvOptions::from_lookup(set("HTTPSTAT_SHOW_SPEED"))
                .unwrap()
                .show_speed
        );
        assert!(
            EnvOptions::from_lookup(set("HTTPSTAT_METRICS_ONLY"))
                .unwrap()
                .metrics_only
        );
        assert!(
            EnvOptions::from_lookup(set("HTTPSTAT_DEBUG"))
                .unwrap()
                .debug
        );
        let off = |k: &str| {
            (k == "HTTPSTAT_SHOW_IP" || k == "HTTPSTAT_SAVE_BODY").then(|| "off".to_string())
        };
        let options = EnvOptions::from_lookup(off).unwrap();
        assert!(!options.show_ip && !options.save_body);
    }

    #[test]
    fn an_invalid_env_value_names_the_variable() {
        let err =
            EnvOptions::from_lookup(|k| (k == "HTTPSTAT_DEBUG").then(|| "banana".to_string()))
                .unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().starts_with("HTTPSTAT_DEBUG:"), "{err}");
    }
}
