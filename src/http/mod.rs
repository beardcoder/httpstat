//! A minimal HTTP/1.1 client that measures each connection phase by hand:
//! DNS lookup, TCP connect, TLS handshake, time-to-first-byte and full transfer.
//!
//! A high-level client is deliberately avoided so that every milestone maps
//! cleanly onto the timing breakdown the tool visualizes. What the client does
//! do is follow the framing rules properly ([`body`]), enforce a real wall-clock
//! budget ([`deadline`]) and refuse to put unvalidated bytes on the wire
//! ([`request`]).

pub mod body;
pub mod deadline;
pub mod headers;
pub mod request;
pub mod response;
pub mod tls;
pub mod uri;

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use url::Url;

use crate::error::{Error, Result};
use crate::timing::Timings;

pub use body::Body;
pub use deadline::Deadline;
pub use headers::Headers;
pub use request::RequestOptions;
pub use response::Head;
pub use tls::Trust;
pub use uri::Target;

/// Read buffer size; large enough that a typical response head arrives in one
/// read, small enough to stay off the stack limit on every platform.
const READ_BUFFER: usize = 16 * 1024;

/// Largest response head we will buffer before declaring the peer hostile.
const MAX_HEAD_BYTES: usize = 256 * 1024;

/// How much of a response body is kept in memory for display and `--save`.
/// Bytes past this point are counted for the transfer rate but dropped, so a
/// large download can be measured without being buffered.
pub const BODY_RETAIN_LIMIT: usize = 8 * 1024 * 1024;

/// Interim (1xx) responses that may precede the real one, bounded so that a
/// server stuck in a `100 Continue` loop cannot hang the client.
const MAX_INTERIM_RESPONSES: usize = 8;

/// One request in a redirect chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub url: String,
    pub status_code: u16,
}

/// The outcome of a (possibly redirect-followed) request.
#[derive(Debug, Clone)]
pub struct HttpResult {
    /// The URL of the final hop, which is what the timings describe.
    pub final_url: String,
    pub https: bool,
    pub method: String,
    pub head: Head,
    pub remote: SocketAddr,
    pub local: SocketAddr,
    pub timings: Timings,
    pub body: Body,
    /// Payload bytes received on the final hop, including any not retained.
    pub download_bytes: usize,
    /// Payload bytes sent on the final hop.
    pub upload_bytes: usize,
    /// Seconds spent between the first response byte and the last.
    pub transfer_secs: f64,
    /// Redirects followed to reach the final hop, in order.
    pub hops: Vec<Hop>,
}

impl HttpResult {
    pub fn status_code(&self) -> u16 {
        self.head.status_code
    }
}

/// Issue the request, following redirects when enabled.
///
/// Reported timings always describe the final hop; the redirect chain is
/// recorded in [`HttpResult::hops`]. The `--max-time` budget covers the whole
/// chain, not each hop separately.
pub fn fetch(url: &str, opts: &RequestOptions) -> Result<HttpResult> {
    opts.validate()?;
    let deadline = Deadline::new(opts.max_time);
    let mut current = uri::normalize(url)?;
    let mut hop_opts = opts.clone();
    let mut hops: Vec<Hop> = Vec::new();

    loop {
        let mut result = fetch_once(&current, &hop_opts, &deadline)?;

        let Some(next) = next_redirect(&current, &result.head, opts)? else {
            result.hops = hops;
            return Ok(result);
        };
        if hops.len() >= opts.max_redirects {
            return Err(Error::request(format!(
                "too many redirects (limit {}), last was {} → {next}",
                opts.max_redirects, current
            )));
        }
        hops.push(Hop {
            url: current.to_string(),
            status_code: result.head.status_code,
        });
        hop_opts = redirected_options(&hop_opts, &result.head, &current, &next);
        current = next;
    }
}

/// Work out where a response redirects to, or `None` when it does not.
fn next_redirect(current: &Url, head: &Head, opts: &RequestOptions) -> Result<Option<Url>> {
    if !opts.follow_redirects || head.redirect_kind().is_none() {
        return Ok(None);
    }
    match head.headers.get("location") {
        // A 3xx without a Location is not actionable; report it as the result.
        None => Ok(None),
        Some(location) => uri::resolve_redirect(current, location).map(Some),
    }
}

/// Adjust method, body and credential headers for the next hop.
fn redirected_options(opts: &RequestOptions, head: &Head, from: &Url, to: &Url) -> RequestOptions {
    let mut next = opts.clone();
    if head.redirect_kind() == Some(response::RedirectKind::ToGet)
        && !next.method.eq_ignore_ascii_case("GET")
        && !next.method.eq_ignore_ascii_case("HEAD")
    {
        next.method = "GET".to_string();
        next.body = None;
        next.headers.remove_all("content-length");
        next.headers.remove_all("content-type");
    }
    if !uri::same_origin(from, to) {
        // Credentials are scoped to the origin they were given for; replaying
        // them to a redirect target would leak them to a third party.
        for header in ["authorization", "cookie", "proxy-authorization"] {
            next.headers.remove_all(header);
        }
    }
    next
}

/// Perform exactly one request/response exchange and time every phase of it.
fn fetch_once(url: &Url, opts: &RequestOptions, deadline: &Deadline) -> Result<HttpResult> {
    let target = uri::target(url)?;
    let start = Instant::now();

    // ---- DNS -------------------------------------------------------------
    // `to_socket_addrs` is a blocking resolver call with no timeout knob in std;
    // the deadline is therefore enforced immediately after it returns.
    deadline.check("DNS resolution")?;
    let addrs: Vec<SocketAddr> = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(|e| {
            Error::request(format!(
                "could not resolve {}:{}: {e}",
                target.host, target.port
            ))
        })?
        .collect();
    let namelookup = start.elapsed();
    if addrs.is_empty() {
        return Err(Error::request(format!(
            "no addresses resolved for {}",
            target.host
        )));
    }
    deadline.check("DNS resolution")?;

    // ---- TCP -------------------------------------------------------------
    let tcp = connect(&addrs, opts.connect_timeout, deadline)?;
    let connected = start.elapsed();
    let local = tcp
        .local_addr()
        .map_err(|e| Error::io("could not read the local socket address", e))?;
    let remote = tcp
        .peer_addr()
        .map_err(|e| Error::io("could not read the peer socket address", e))?;
    // Timing a single small request means Nagle's algorithm would only add delay.
    let _ = tcp.set_nodelay(true);

    // ---- TLS -------------------------------------------------------------
    let mut conn = if target.https {
        Conn::Tls(Box::new(handshake(tcp, &target, opts, deadline)?))
    } else {
        Conn::Plain(tcp)
    };
    let pretransfer = start.elapsed();

    // ---- Request ---------------------------------------------------------
    let wire = request::build(url, &target, opts);
    conn.set_write_timeout(deadline.remaining())?;
    deadline.check("sending the request")?;
    conn.write_all(&wire)
        .and_then(|()| conn.flush())
        .map_err(|e| map_io("sending the request", e, deadline))?;

    // ---- Response --------------------------------------------------------
    let exchange = read_response(&mut conn, &opts.method, deadline, start)?;
    let total = start.elapsed();
    let starttransfer = exchange.first_byte.unwrap_or(total);

    Ok(HttpResult {
        final_url: url.to_string(),
        https: target.https,
        method: opts.method.clone(),
        download_bytes: exchange.body.total,
        upload_bytes: opts.body.as_ref().map_or(0, Vec::len),
        body: exchange.body,
        head: exchange.head,
        remote,
        local,
        timings: Timings::from_durations(namelookup, connected, pretransfer, starttransfer, total),
        transfer_secs: total.saturating_sub(starttransfer).as_secs_f64(),
        hops: Vec::new(),
    })
}

/// Connect to the first address that accepts us.
///
/// Trying every resolved address in turn is what lets a host that advertises an
/// unreachable AAAA record still connect over its working IPv4 address.
fn connect(
    addrs: &[SocketAddr],
    connect_timeout: Option<Duration>,
    deadline: &Deadline,
) -> Result<TcpStream> {
    let mut last: Option<(SocketAddr, io::Error)> = None;
    for addr in addrs {
        deadline.check("the TCP connection")?;
        // Each attempt gets the smaller of --connect-timeout and what is left of
        // --max-time, so N unreachable addresses cannot multiply the budget.
        let attempt = match deadline.cap(connect_timeout) {
            Some(limit) if limit.is_zero() => {
                return Err(deadline.timeout_error("the TCP connection"))
            }
            Some(limit) => TcpStream::connect_timeout(addr, limit),
            None => TcpStream::connect(*addr),
        };
        match attempt {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some((*addr, e)),
        }
    }
    let (addr, error) = last.expect("addrs is non-empty");
    if is_timeout(&error) && deadline.expired() {
        return Err(deadline.timeout_error("the TCP connection"));
    }
    Err(Error::request(format!(
        "could not connect to {addr}: {error}"
    )))
}

/// Drive the TLS handshake to completion within the deadline.
fn handshake(
    tcp: TcpStream,
    target: &Target,
    opts: &RequestOptions,
    deadline: &Deadline,
) -> Result<StreamOwned<ClientConnection, TcpStream>> {
    let server_name = ServerName::try_from(target.host.clone())
        .map_err(|_| Error::request(format!("invalid TLS server name: {}", target.host)))?
        .to_owned();
    let trust = tls::Trust::resolve(opts.insecure, opts.ca_file.as_deref());
    let mut client = ClientConnection::new(tls::config(&trust)?, server_name)
        .map_err(|e| Error::request(format!("TLS setup failed: {e}")))?;

    set_timeouts(&tcp, deadline.remaining())?;
    let mut sock = tcp;
    while client.is_handshaking() {
        deadline.check("the TLS handshake")?;
        set_timeouts(&sock, deadline.remaining())?;
        client
            .complete_io(&mut sock)
            .map_err(|e| map_io("the TLS handshake", e, deadline))?;
    }
    Ok(StreamOwned::new(client, sock))
}

/// A response head plus its decoded body.
struct Exchange {
    head: Head,
    body: Body,
    first_byte: Option<Duration>,
}

/// Read one response: head, then the body according to its framing.
fn read_response(
    conn: &mut Conn,
    method: &str,
    deadline: &Deadline,
    start: Instant,
) -> Result<Exchange> {
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut pending: Vec<u8> = Vec::new();
    let mut first_byte: Option<Duration> = None;

    for _ in 0..=MAX_INTERIM_RESPONSES {
        // Read until the head is complete, keeping whatever body bytes tag along.
        let split = loop {
            if let Some(at) = response::find_head_end(&pending) {
                break at;
            }
            if pending.len() > MAX_HEAD_BYTES {
                return Err(Error::request(format!(
                    "malformed response: header block exceeds {} KiB",
                    MAX_HEAD_BYTES / 1024
                )));
            }
            let n = read(conn, &mut buffer, deadline)?;
            if n == 0 {
                return Err(Error::request(if pending.is_empty() {
                    "the server closed the connection without sending a response".to_string()
                } else {
                    "the server closed the connection mid-header".to_string()
                }));
            }
            if first_byte.is_none() {
                first_byte = Some(start.elapsed());
            }
            pending.extend_from_slice(&buffer[..n]);
        };

        let head = response::parse_head(&pending[..split])?;
        let rest = pending.split_off(split + 4);
        pending = rest;

        // A 1xx is a placeholder: the real response follows on the same stream.
        if head.is_interim() {
            continue;
        }

        let framing = body::framing_for(method, head.status_code, &head.headers)?;
        let mut reader = body::BodyReader::new(framing, BODY_RETAIN_LIMIT);
        let mut done = reader.push(&pending)?;
        while !done {
            let n = read(conn, &mut buffer, deadline)?;
            if n == 0 {
                return Ok(Exchange {
                    head,
                    body: reader.finish_at_eof()?,
                    first_byte,
                });
            }
            done = reader.push(&buffer[..n])?;
        }
        return Ok(Exchange {
            head,
            body: reader.finish(),
            first_byte,
        });
    }

    Err(Error::request(format!(
        "the server sent more than {MAX_INTERIM_RESPONSES} interim (1xx) responses"
    )))
}

/// One read, bounded by the deadline. `Ok(0)` means the peer closed the stream.
fn read(conn: &mut Conn, buffer: &mut [u8], deadline: &Deadline) -> Result<usize> {
    deadline.check("reading the response")?;
    conn.set_read_timeout(deadline.remaining())?;
    match conn.read(buffer) {
        Ok(n) => Ok(n),
        // Many servers drop the TCP connection without a TLS close_notify. That
        // is an unclean shutdown, not a transport error: the framing layer
        // decides whether the body we got is complete.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
        Err(e) => Err(map_io("reading the response", e, deadline)),
    }
}

fn map_io(phase: &str, error: io::Error, deadline: &Deadline) -> Error {
    if is_timeout(&error) {
        return deadline.timeout_error(phase);
    }
    Error::request(format!("{phase} failed: {error}"))
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// Apply a timeout to a socket, treating "no timeout" as blocking.
fn set_timeouts(tcp: &TcpStream, timeout: Option<Duration>) -> Result<()> {
    // A zero Duration means "no timeout" to the OS, which is the opposite of
    // what an expired deadline means, so it is clamped to the smallest tick.
    let timeout = timeout.map(|d| d.max(Duration::from_millis(1)));
    tcp.set_read_timeout(timeout)
        .and_then(|()| tcp.set_write_timeout(timeout))
        .map_err(|e| Error::io("could not configure the socket timeout", e))
}

/// A connection that may or may not be wrapped in TLS, behind one `Read`/`Write`
/// surface.
enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Conn {
    fn socket(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => &s.sock,
        }
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        let timeout = timeout.map(|d| d.max(Duration::from_millis(1)));
        self.socket()
            .set_read_timeout(timeout)
            .map_err(|e| Error::io("could not configure the socket read timeout", e))
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        let timeout = timeout.map(|d| d.max(Duration::from_millis(1)));
        self.socket()
            .set_write_timeout(timeout)
            .map_err(|e| Error::io("could not configure the socket write timeout", e))
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::response::parse_head;

    fn head(raw: &str) -> Head {
        parse_head(raw.as_bytes()).unwrap()
    }

    fn following() -> RequestOptions {
        RequestOptions {
            follow_redirects: true,
            ..RequestOptions::default()
        }
    }

    #[test]
    fn redirects_are_only_followed_when_asked_for() {
        let url = uri::normalize("http://example.com/a").unwrap();
        let head = head("HTTP/1.1 302 Found\r\nLocation: /b");
        assert!(next_redirect(&url, &head, &RequestOptions::default())
            .unwrap()
            .is_none());
        assert_eq!(
            next_redirect(&url, &head, &following())
                .unwrap()
                .unwrap()
                .as_str(),
            "http://example.com/b"
        );
    }

    #[test]
    fn a_redirect_without_a_location_is_a_final_response() {
        let url = uri::normalize("http://example.com/a").unwrap();
        let head = head("HTTP/1.1 302 Found\r\nX-Nothing: here");
        assert!(next_redirect(&url, &head, &following()).unwrap().is_none());
    }

    #[test]
    fn a_redirect_to_an_unsupported_scheme_is_rejected() {
        let url = uri::normalize("http://example.com/a").unwrap();
        let head = head("HTTP/1.1 302 Found\r\nLocation: ftp://example.com/x");
        assert!(next_redirect(&url, &head, &following()).is_err());
    }

    #[test]
    fn a_303_turns_a_post_into_a_bodyless_get() {
        let from = uri::normalize("http://example.com/a").unwrap();
        let to = uri::normalize("http://example.com/b").unwrap();
        let opts = RequestOptions {
            method: "POST".into(),
            body: Some(b"payload".to_vec()),
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())]
                .into_iter()
                .collect(),
            ..following()
        };
        let next = redirected_options(&opts, &head("HTTP/1.1 303 See Other"), &from, &to);
        assert_eq!(next.method, "GET");
        assert!(next.body.is_none());
        assert!(!next.headers.contains("content-type"));
    }

    #[test]
    fn a_307_replays_the_method_and_body() {
        let from = uri::normalize("http://example.com/a").unwrap();
        let to = uri::normalize("http://example.com/b").unwrap();
        let opts = RequestOptions {
            method: "POST".into(),
            body: Some(b"payload".to_vec()),
            ..following()
        };
        let next = redirected_options(&opts, &head("HTTP/1.1 307 Temporary Redirect"), &from, &to);
        assert_eq!(next.method, "POST");
        assert_eq!(next.body.as_deref(), Some(&b"payload"[..]));
    }

    #[test]
    fn credentials_are_dropped_when_a_redirect_crosses_origins() {
        let from = uri::normalize("https://example.com/a").unwrap();
        let opts = RequestOptions {
            headers: vec![
                ("Authorization".to_string(), "Bearer secret".to_string()),
                ("Cookie".to_string(), "session=1".to_string()),
                ("X-Trace".to_string(), "keep-me".to_string()),
            ]
            .into_iter()
            .collect(),
            ..following()
        };
        let head = head("HTTP/1.1 302 Found");

        let same = uri::normalize("https://example.com/b").unwrap();
        let kept = redirected_options(&opts, &head, &from, &same);
        assert!(kept.headers.contains("authorization"));

        let elsewhere = uri::normalize("https://evil.test/b").unwrap();
        let stripped = redirected_options(&opts, &head, &from, &elsewhere);
        assert!(!stripped.headers.contains("authorization"));
        assert!(!stripped.headers.contains("cookie"));
        assert!(stripped.headers.contains("x-trace"));
    }

    #[test]
    fn invalid_options_are_rejected_before_any_connection_is_made() {
        let opts = RequestOptions {
            method: "BAD METHOD".into(),
            ..RequestOptions::default()
        };
        let err = fetch("http://127.0.0.1:1/", &opts).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
    }
}
