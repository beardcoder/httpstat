//! Response status-line and header parsing.

use crate::error::{Error, Result};
use crate::http::headers::Headers;

/// The parsed head of a response, before the body is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The status line as sent, e.g. `HTTP/1.1 200 OK`.
    pub status_line: String,
    /// The protocol version, e.g. `1.1`.
    pub version: String,
    pub status_code: u16,
    pub reason: String,
    pub headers: Headers,
}

impl Head {
    /// Whether this is an interim (1xx) response that precedes the real one.
    /// `101 Switching Protocols` is final as far as this client is concerned.
    pub fn is_interim(&self) -> bool {
        matches!(self.status_code, 100..=199) && self.status_code != 101
    }

    /// Whether the status invites a redirect, and with which method semantics.
    pub fn redirect_kind(&self) -> Option<RedirectKind> {
        match self.status_code {
            // 303 always continues with GET; 301/302 do so for anything that is
            // not already GET/HEAD, as every browser and curl do in practice.
            301..=303 => Some(RedirectKind::ToGet),
            307 | 308 => Some(RedirectKind::PreserveMethod),
            _ => None,
        }
    }
}

/// How a redirect affects the method and body of the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// Continue with GET and drop the request body.
    ToGet,
    /// Replay the same method and body.
    PreserveMethod,
}

/// Locate the end of the header block (the CRLF CRLF separator).
/// Returns the index of the separator, so the body starts four bytes later.
pub fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the header block of a response (everything before `\r\n\r\n`).
pub fn parse_head(raw: &[u8]) -> Result<Head> {
    let text = String::from_utf8_lossy(raw);
    let mut lines = text.split("\r\n");

    let status_line = lines
        .next()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| Error::request("malformed response: empty status line".to_string()))?
        .to_string();

    let rest = status_line.strip_prefix("HTTP/").ok_or_else(|| {
        Error::request(format!(
            "malformed response: expected an HTTP status line, got {:?}",
            truncate(&status_line, 60)
        ))
    })?;
    let mut parts = rest.splitn(3, ' ');
    let version = parts.next().unwrap_or_default().to_string();
    let code = parts.next().unwrap_or_default();
    let status_code: u16 = code.parse().map_err(|_| {
        Error::request(format!(
            "malformed response: invalid status code {:?}",
            truncate(code, 20)
        ))
    })?;
    let reason = parts.next().unwrap_or_default().trim().to_string();

    let mut headers = Headers::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        // Obsolete line folding: a leading space or tab continues the previous
        // field value (RFC 9112 §5.2 — deprecated, but still seen in the wild).
        if line.starts_with([' ', '\t']) {
            if headers.append_to_last(line.trim()) {
                continue;
            }
            return Err(Error::request(
                "malformed response: header continuation before any header".to_string(),
            ));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| {
            Error::request(format!(
                "malformed response: header line without a colon: {:?}",
                truncate(line, 60)
            ))
        })?;
        let name = name.trim_end();
        if name.is_empty() {
            return Err(Error::request(
                "malformed response: header line with an empty name".to_string(),
            ));
        }
        headers.push(name, value.trim());
    }

    Ok(Head {
        status_line,
        version,
        status_code,
        reason,
        headers,
    })
}

fn truncate(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let head: String = s.chars().take(limit).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_line_and_headers() {
        let head =
            parse_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nServer: nginx").unwrap();
        assert_eq!(head.status_line, "HTTP/1.1 200 OK");
        assert_eq!(head.version, "1.1");
        assert_eq!(head.status_code, 200);
        assert_eq!(head.reason, "OK");
        assert_eq!(head.headers.get("content-type"), Some("text/html"));
        assert_eq!(head.headers.len(), 2);
    }

    #[test]
    fn parses_a_status_line_without_a_reason_phrase() {
        let head = parse_head(b"HTTP/1.0 204").unwrap();
        assert_eq!(head.status_code, 204);
        assert_eq!(head.reason, "");
        assert_eq!(head.version, "1.0");
    }

    #[test]
    fn keeps_repeated_headers_and_folds_continuations() {
        let head = parse_head(
            b"HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nX-Long: one\r\n\ttwo",
        )
        .unwrap();
        assert_eq!(
            head.headers.get_all("set-cookie").collect::<Vec<_>>(),
            ["a=1", "b=2"]
        );
        assert_eq!(head.headers.get("x-long"), Some("one two"));
    }

    #[test]
    fn rejects_responses_that_are_not_http() {
        let err = parse_head(b"<html>oops</html>\r\n").unwrap_err();
        assert!(
            err.to_string().contains("expected an HTTP status line"),
            "{err}"
        );
        assert!(parse_head(b"").is_err());
        assert!(parse_head(b"HTTP/1.1 not-a-code OK").is_err());
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nbroken-header-line").is_err());
        assert!(parse_head(b"HTTP/1.1 200 OK\r\n: empty-name").is_err());
    }

    #[test]
    fn interim_responses_are_recognised() {
        assert!(parse_head(b"HTTP/1.1 100 Continue").unwrap().is_interim());
        assert!(!parse_head(b"HTTP/1.1 101 Switching Protocols")
            .unwrap()
            .is_interim());
        assert!(!parse_head(b"HTTP/1.1 200 OK").unwrap().is_interim());
    }

    #[test]
    fn redirect_kinds_follow_the_status_code() {
        let kind = |code: u16| {
            parse_head(format!("HTTP/1.1 {code} X").as_bytes())
                .unwrap()
                .redirect_kind()
        };
        assert_eq!(kind(301), Some(RedirectKind::ToGet));
        assert_eq!(kind(303), Some(RedirectKind::ToGet));
        assert_eq!(kind(307), Some(RedirectKind::PreserveMethod));
        assert_eq!(kind(308), Some(RedirectKind::PreserveMethod));
        assert_eq!(kind(200), None);
        assert_eq!(kind(304), None);
    }

    #[test]
    fn finds_the_head_terminator() {
        assert_eq!(find_head_end(b"HTTP/1.1 200 OK\r\n\r\nbody"), Some(15));
        assert_eq!(find_head_end(b"HTTP/1.1 200 OK\r\n"), None);
    }
}
