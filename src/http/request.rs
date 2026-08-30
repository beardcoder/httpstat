//! Request construction and validation.
//!
//! Header names and values are validated before a request is built. Without
//! that check a value containing CR/LF would splice extra header lines — or a
//! whole second request — into the connection (request smuggling), because the
//! request is assembled as text.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::http::headers::Headers;
use crate::http::uri::{self, Target};
use url::Url;

/// How a single request should be issued.
#[derive(Debug, Clone)]
pub struct RequestOptions {
    pub method: String,
    /// Caller-supplied headers; these replace same-named defaults.
    pub headers: Headers,
    pub body: Option<Vec<u8>>,
    pub follow_redirects: bool,
    pub max_redirects: usize,
    pub insecure: bool,
    /// PEM bundle to verify against instead of the roots built into the binary.
    pub ca_file: Option<PathBuf>,
    pub user_agent: String,
    pub connect_timeout: Option<Duration>,
    pub max_time: Option<Duration>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        RequestOptions {
            method: "GET".to_string(),
            headers: Headers::new(),
            body: None,
            follow_redirects: false,
            max_redirects: 10,
            insecure: false,
            ca_file: None,
            user_agent: crate::USER_AGENT.to_string(),
            connect_timeout: None,
            max_time: None,
        }
    }
}

impl RequestOptions {
    /// Reject anything that could corrupt the request we are about to write.
    pub fn validate(&self) -> Result<()> {
        validate_method(&self.method)?;
        for (name, value) in self.headers.iter() {
            validate_header(name, value)?;
        }
        validate_header_value("User-Agent", &self.user_agent)?;
        for (label, duration) in [
            ("--connect-timeout", self.connect_timeout),
            ("--max-time", self.max_time),
        ] {
            if let Some(d) = duration {
                if d.is_zero() {
                    return Err(Error::usage(format!("{label} must be greater than zero")));
                }
            }
        }
        Ok(())
    }
}

/// Serialize a request into the bytes to put on the wire.
pub fn build(url: &Url, target: &Target, opts: &RequestOptions) -> Vec<u8> {
    let mut head = format!("{} {} HTTP/1.1\r\n", opts.method, uri::request_target(url));
    for (name, value) in effective_headers(target, opts).iter() {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");

    let mut bytes = head.into_bytes();
    if let Some(body) = &opts.body {
        bytes.extend_from_slice(body);
    }
    bytes
}

/// Merge the defaults with the caller's headers.
///
/// A caller-supplied header replaces the default of the same name (so
/// `-H 'User-Agent: x'` sends one User-Agent, not two), while repeats among the
/// caller's own headers are all preserved — `Cookie` and `Accept` are legally
/// repeatable. `Connection: close` is always sent: the client issues exactly one
/// request per connection and relies on the close to delimit unframed bodies.
pub fn effective_headers(target: &Target, opts: &RequestOptions) -> Headers {
    let mut defaults = Headers::new();
    defaults.push("Host", target.host_header());
    defaults.push("User-Agent", opts.user_agent.clone());
    defaults.push("Accept", "*/*");
    // We do not decompress, so ask for the identity coding; a caller who
    // overrides this gets the raw compressed bytes as the body.
    defaults.push("Accept-Encoding", "identity");
    if let Some(body) = &opts.body {
        defaults.push("Content-Length", body.len().to_string());
    }

    let mut headers = Headers::new();
    for (name, value) in defaults.iter() {
        if !opts.headers.contains(name) {
            headers.push(name.clone(), value.clone());
        }
    }
    for (name, value) in opts.headers.iter() {
        if name.eq_ignore_ascii_case("connection") {
            continue;
        }
        // Without a body a caller-supplied Content-Length would desynchronize
        // the connection, so it is dropped rather than honoured.
        if opts.body.is_none() && name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        headers.push(name.clone(), value.clone());
    }
    headers.push("Connection", "close");
    headers
}

/// Validate a `Name: Value` pair against the RFC 9110 grammar.
pub fn validate_header(name: &str, value: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::usage("header name must not be empty"));
    }
    if !name.chars().all(is_token_char) {
        return Err(Error::usage(format!(
            "invalid header name \"{name}\": only token characters are allowed"
        )));
    }
    validate_header_value(name, value)
}

fn validate_header_value(name: &str, value: &str) -> Result<()> {
    // Rejecting control characters — CR and LF above all — is what stops header
    // injection through `-H` or `--user-agent`. Non-ASCII text is passed through
    // as UTF-8 like curl does, since servers treat field values as opaque bytes.
    if let Some(bad) = value.chars().find(|c| *c != '\t' && c.is_control()) {
        return Err(Error::usage(format!(
            "invalid value for header \"{name}\": contains {}",
            describe(bad)
        )));
    }
    Ok(())
}

/// Validate an HTTP method token.
pub fn validate_method(method: &str) -> Result<()> {
    if method.is_empty() {
        return Err(Error::usage("HTTP method must not be empty"));
    }
    if !method.chars().all(is_token_char) {
        return Err(Error::usage(format!(
            "invalid HTTP method \"{method}\": only token characters are allowed"
        )));
    }
    Ok(())
}

/// RFC 9110 `tchar`.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}

fn describe(c: char) -> String {
    match c {
        '\r' => "a carriage return".to_string(),
        '\n' => "a line feed".to_string(),
        '\0' => "a NUL byte".to_string(),
        c => format!("the control character {:#04x}", c as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_with(headers: &[(&str, &str)]) -> RequestOptions {
        RequestOptions {
            user_agent: "httpstat-test".into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..RequestOptions::default()
        }
    }

    fn render(url: &str, opts: &RequestOptions) -> String {
        let url = uri::normalize(url).unwrap();
        let target = uri::target(&url).unwrap();
        String::from_utf8(build(&url, &target, opts)).unwrap()
    }

    #[test]
    fn a_plain_get_has_the_expected_request_line_and_defaults() {
        let text = render("https://example.com/a?b=1", &opts_with(&[]));
        let mut lines = text.split("\r\n");
        assert_eq!(lines.next(), Some("GET /a?b=1 HTTP/1.1"));
        assert!(text.contains("Host: example.com\r\n"));
        assert!(text.contains("User-Agent: httpstat-test\r\n"));
        assert!(text.contains("Accept: */*\r\n"));
        assert!(text.contains("Accept-Encoding: identity\r\n"));
        assert!(text.ends_with("Connection: close\r\n\r\n"));
        assert!(!text.contains("Content-Length"));
    }

    #[test]
    fn a_non_default_port_appears_in_the_host_header() {
        let text = render("http://example.com:8080/", &opts_with(&[]));
        assert!(text.contains("Host: example.com:8080\r\n"), "{text}");
    }

    #[test]
    fn caller_headers_replace_defaults_instead_of_duplicating_them() {
        let text = render(
            "https://example.com/",
            &opts_with(&[("User-Agent", "mine/1.0"), ("Accept", "application/json")]),
        );
        assert_eq!(text.matches("User-Agent:").count(), 1);
        assert_eq!(text.matches("Accept:").count(), 1);
        assert!(text.contains("User-Agent: mine/1.0\r\n"));
        assert!(text.contains("Accept: application/json\r\n"));
    }

    #[test]
    fn repeated_caller_headers_are_all_sent() {
        let text = render(
            "https://example.com/",
            &opts_with(&[("Cookie", "a=1"), ("Cookie", "b=2")]),
        );
        assert!(text.contains("Cookie: a=1\r\n"));
        assert!(text.contains("Cookie: b=2\r\n"));
    }

    #[test]
    fn connection_close_is_always_sent_exactly_once() {
        let text = render(
            "https://example.com/",
            &opts_with(&[("Connection", "keep-alive")]),
        );
        assert_eq!(text.matches("Connection:").count(), 1);
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn a_body_gets_a_content_length_and_is_appended() {
        let opts = RequestOptions {
            method: "POST".into(),
            body: Some(b"{\"a\":1}".to_vec()),
            ..opts_with(&[])
        };
        let text = render("https://example.com/x", &opts);
        assert!(text.starts_with("POST /x HTTP/1.1\r\n"));
        assert!(text.contains("Content-Length: 7\r\n"));
        assert!(text.ends_with("\r\n\r\n{\"a\":1}"));
    }

    #[test]
    fn a_content_length_without_a_body_is_dropped() {
        let text = render(
            "https://example.com/",
            &opts_with(&[("Content-Length", "99")]),
        );
        assert!(!text.contains("Content-Length"), "{text}");
    }

    #[test]
    fn header_injection_through_crlf_is_rejected() {
        for value in ["a\r\nX-Evil: 1", "a\nX-Evil: 1", "a\rb", "a\0b"] {
            let opts = opts_with(&[("X-Test", value)]);
            assert!(
                opts.validate().is_err(),
                "expected {value:?} to be rejected"
            );
        }
        let opts = RequestOptions {
            user_agent: "evil\r\nX-Evil: 1".into(),
            ..RequestOptions::default()
        };
        assert!(opts.validate().is_err());
    }

    #[test]
    fn invalid_header_names_and_methods_are_rejected() {
        assert!(validate_header("X Test", "1").is_err());
        assert!(validate_header("", "1").is_err());
        assert!(validate_header("X-Test", "fine").is_ok());
        // Non-ASCII values are opaque bytes to a server, not an injection risk.
        assert!(validate_header("X-Test", "grüße").is_ok());
        assert!(validate_method("GET").is_ok());
        assert!(validate_method("PROPFIND").is_ok());
        assert!(validate_method("GET /evil HTTP/1.1").is_err());
        assert!(validate_method("").is_err());
    }

    #[test]
    fn zero_timeouts_are_rejected() {
        let opts = RequestOptions {
            max_time: Some(Duration::ZERO),
            ..RequestOptions::default()
        };
        assert!(opts.validate().is_err());
    }
}
