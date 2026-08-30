//! URL normalization and redirect resolution.

use url::Url;

use crate::error::{Error, Result};

/// The origin of a request target: what we connect to and what we send as `Host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub https: bool,
    pub host: String,
    pub port: u16,
}

impl Target {
    /// The `Host` header value: the port is omitted when it is the scheme
    /// default, and an IPv6 literal is bracketed so the colons cannot be read
    /// as a port separator.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default_port = if self.https { 443 } else { 80 };
        if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// Accept bare hosts like the original tool (`example.com` → `http://example.com`)
/// while rejecting anything that is not an absolute http(s) URL.
pub fn normalize(input: &str) -> Result<Url> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::usage("empty URL"));
    }
    // Only prefix a scheme for genuinely scheme-less input. `mailto:x` parses as
    // a URL with a scheme we do not support, and must not become
    // `http://mailto:x`.
    let candidate = if has_scheme(trimmed) {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let url = Url::parse(&candidate)
        .map_err(|e| Error::usage(format!("invalid URL \"{input}\": {e}")))?;
    check_supported(&url)?;
    Ok(url)
}

/// Resolve a `Location` header against the URL it was received from.
pub fn resolve_redirect(base: &Url, location: &str) -> Result<Url> {
    let location = location.trim();
    if location.is_empty() {
        return Err(Error::request("redirect response has an empty Location"));
    }
    let url = base
        .join(location)
        .map_err(|e| Error::request(format!("invalid redirect Location \"{location}\": {e}")))?;
    check_supported(&url)?;
    Ok(url)
}

/// Split a URL into the connection target, rejecting anything unroutable.
pub fn target(url: &Url) -> Result<Target> {
    let https = match url.scheme() {
        "https" => true,
        "http" => false,
        other => {
            return Err(Error::request(format!(
                "unsupported URL scheme \"{other}\", only http and https are supported"
            )))
        }
    };
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| Error::usage(format!("URL has no host: {url}")))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| Error::usage(format!("could not determine a port for {url}")))?;
    Ok(Target {
        https,
        // `url` keeps IPv6 literals bracketed; sockets and SNI want them bare.
        host: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port,
    })
}

/// The request-target sent on the request line: path plus query, never empty.
pub fn request_target(url: &Url) -> String {
    let path = url.path();
    let mut target = if path.is_empty() { "/" } else { path }.to_string();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    target
}

/// Whether two URLs address the same origin (scheme, host and port all equal).
/// Credentials must not be replayed across an origin change.
pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str().map(str::to_ascii_lowercase) == b.host_str().map(str::to_ascii_lowercase)
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Whether the input already carries a URL scheme.
///
/// A scheme is `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`, but that alone
/// would read `localhost:8080` as the scheme `localhost` — so a scheme-shaped
/// prefix only counts when it is followed by `//`, or by something that cannot
/// be a port number. `http://x` and `mailto:a@b.c` have schemes;
/// `localhost:8080` and `example.com` do not.
fn has_scheme(input: &str) -> bool {
    let Some(colon) = input.find(':') else {
        return false;
    };
    let (scheme, rest) = input.split_at(colon);
    let scheme_shaped = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_shaped {
        return false;
    }
    let after_colon = &rest[1..];
    after_colon.starts_with("//") || !after_colon.starts_with(|c: char| c.is_ascii_digit())
}

fn check_supported(url: &Url) -> Result<()> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(Error::usage(format!(
            "unsupported URL scheme \"{other}\", only http and https are supported"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_hosts_default_to_http() {
        assert_eq!(
            normalize("example.com").unwrap().as_str(),
            "http://example.com/"
        );
        assert_eq!(
            normalize("https://example.com/x").unwrap().as_str(),
            "https://example.com/x"
        );
    }

    #[test]
    fn bare_host_with_a_port_is_not_mistaken_for_a_scheme() {
        for input in ["127.0.0.1:8080/health", "localhost:8080/health"] {
            let url = normalize(input).unwrap();
            assert_eq!(url.scheme(), "http", "{input}");
            assert_eq!(url.port(), Some(8080), "{input}");
            assert_eq!(request_target(&url), "/health", "{input}");
        }
        let url = normalize("[::1]:9000").unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.port(), Some(9000));
    }

    #[test]
    fn unsupported_schemes_are_rejected() {
        for input in ["ftp://example.com", "file:///etc/passwd", "mailto:a@b.c"] {
            assert!(normalize(input).is_err(), "expected {input} to be rejected");
        }
        assert!(normalize("   ").is_err());
    }

    #[test]
    fn targets_carry_the_default_port_and_unbracketed_host() {
        let t = target(&normalize("https://example.com").unwrap()).unwrap();
        assert_eq!(
            t,
            Target {
                https: true,
                host: "example.com".into(),
                port: 443
            }
        );
        assert_eq!(t.host_header(), "example.com");

        let t = target(&normalize("http://example.com:8080").unwrap()).unwrap();
        assert_eq!(t.host_header(), "example.com:8080");

        let t = target(&normalize("http://[::1]:9000/").unwrap()).unwrap();
        assert_eq!(
            t.host, "::1",
            "sockets and SNI want the address unbracketed"
        );
        assert_eq!(t.port, 9000);
        assert_eq!(
            t.host_header(),
            "[::1]:9000",
            "the Host header wants it bracketed"
        );

        let t = target(&normalize("http://[::1]/").unwrap()).unwrap();
        assert_eq!(t.host_header(), "[::1]");
    }

    #[test]
    fn request_target_includes_the_query_and_defaults_to_slash() {
        let url = normalize("http://example.com").unwrap();
        assert_eq!(request_target(&url), "/");
        let url = normalize("http://example.com/a/b?x=1&y=2").unwrap();
        assert_eq!(request_target(&url), "/a/b?x=1&y=2");
    }

    #[test]
    fn redirects_resolve_relative_absolute_and_protocol_relative_targets() {
        let base = normalize("https://example.com/a/b").unwrap();
        assert_eq!(
            resolve_redirect(&base, "/c").unwrap().as_str(),
            "https://example.com/c"
        );
        assert_eq!(
            resolve_redirect(&base, "d").unwrap().as_str(),
            "https://example.com/a/d"
        );
        assert_eq!(
            resolve_redirect(&base, "//other.test/e").unwrap().as_str(),
            "https://other.test/e"
        );
        assert!(resolve_redirect(&base, "  ").is_err());
        assert!(resolve_redirect(&base, "gopher://example.com").is_err());
    }

    #[test]
    fn same_origin_compares_scheme_host_and_effective_port() {
        let a = normalize("https://example.com/a").unwrap();
        assert!(same_origin(
            &a,
            &normalize("https://EXAMPLE.com:443/b").unwrap()
        ));
        assert!(!same_origin(
            &a,
            &normalize("http://example.com/a").unwrap()
        ));
        assert!(!same_origin(&a, &normalize("https://other.com/a").unwrap()));
    }
}
