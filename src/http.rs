//! A minimal HTTP/1.1 client that measures each connection phase by hand:
//! DNS lookup, TCP connect, TLS handshake, time-to-first-byte and full transfer.
//!
//! This intentionally avoids a high-level client so every milestone maps cleanly
//! onto the timing breakdown the tool visualises.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConnection, DigitallySignedStruct, Error as TlsError, SignatureScheme, StreamOwned,
};
use url::Url;

use crate::timing::Timings;

const MAX_REDIRECTS: usize = 10;

/// How a single request should be issued.
pub struct RequestOptions {
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub follow_redirects: bool,
    pub insecure: bool,
    pub user_agent: String,
    pub connect_timeout: Option<Duration>,
    pub max_time: Option<Duration>,
}

/// Parsed response metadata (everything except the body).
pub struct ResponseInfo {
    pub status_line: String,
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
    pub remote_ip: String,
    pub remote_port: String,
    pub local_ip: String,
    pub local_port: String,
}

/// The outcome of a (possibly redirect-followed) request.
pub struct HttpResult {
    pub final_url: String,
    pub timings: Timings,
    pub response: ResponseInfo,
    pub body: Vec<u8>,
    pub download_bytes: usize,
    pub transfer_secs: f64,
}

/// A connection that may or may not be wrapped in TLS, exposing a uniform
/// `Read`/`Write` surface.
enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
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

/// Certificate verifier used for `-k/--insecure`: accepts any certificate but
/// still validates signature schemes via the active crypto provider.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config(insecure: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = if insecure {
        rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("ring provider supports default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring provider supports default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// Issue the request, transparently following up to [`MAX_REDIRECTS`] redirects
/// when enabled. Reported timings always reflect the final hop.
pub fn fetch(url: &str, opts: &RequestOptions) -> Result<HttpResult, String> {
    let mut current = normalize_url(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let result = fetch_once(&current, opts)?;
        if opts.follow_redirects && is_redirect(result.response.status_code) {
            if let Some(location) = header_value(&result.response.headers, "location") {
                current = current
                    .join(&location)
                    .map_err(|e| format!("invalid redirect Location \"{location}\": {e}"))?;
                continue;
            }
        }
        return Ok(result);
    }
    Err(format!("too many redirects (>{MAX_REDIRECTS})"))
}

fn fetch_once(url: &Url, opts: &RequestOptions) -> Result<HttpResult, String> {
    let scheme = url.scheme();
    let https = match scheme {
        "https" => true,
        "http" => false,
        other => return Err(format!("unsupported scheme: {other}")),
    };
    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "could not determine port".to_string())?;

    let start = Instant::now();

    // DNS lookup.
    let addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed for {host}:{port}: {e}"))?
        .collect();
    let namelookup = start.elapsed();
    if addrs.is_empty() {
        return Err(format!("no addresses resolved for {host}"));
    }

    // TCP connect — try each resolved address in turn so a host that advertises
    // an unreachable AAAA record still connects over its working IPv4 address.
    let mut tcp = None;
    let mut last_err = None;
    for addr in &addrs {
        let attempt = match opts.connect_timeout {
            Some(d) => TcpStream::connect_timeout(addr, d),
            None => TcpStream::connect(*addr),
        };
        match attempt {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(e) => last_err = Some((*addr, e)),
        }
    }
    let tcp = tcp.ok_or_else(|| {
        let (addr, e) = last_err.expect("addrs is non-empty");
        format!("TCP connect to {addr} failed: {e}")
    })?;
    let connect = start.elapsed();

    let local = tcp.local_addr().map_err(|e| e.to_string())?;
    let remote = tcp.peer_addr().map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    if let Some(d) = opts.max_time {
        let _ = tcp.set_read_timeout(Some(d));
        let _ = tcp.set_write_timeout(Some(d));
    }

    // TLS handshake (https only); for plain http pretransfer equals connect.
    let mut conn = if https {
        let server_name = ServerName::try_from(host.clone())
            .map_err(|_| format!("invalid TLS server name: {host}"))?;
        let mut tls = ClientConnection::new(tls_config(opts.insecure), server_name)
            .map_err(|e| format!("TLS setup failed: {e}"))?;
        let mut sock = tcp;
        while tls.is_handshaking() {
            tls.complete_io(&mut sock)
                .map_err(|e| format!("TLS handshake failed: {e}"))?;
        }
        Conn::Tls(Box::new(StreamOwned::new(tls, sock)))
    } else {
        Conn::Plain(tcp)
    };
    let pretransfer = start.elapsed();

    // Send request.
    let request = build_request(url, &host, opts);
    conn.write_all(&request)
        .and_then(|_| conn.flush())
        .map_err(|e| format!("failed sending request: {e}"))?;

    // Read response, recording time-to-first-byte and full transfer time.
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let mut first_byte: Option<Duration> = None;
    loop {
        match conn.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if first_byte.is_none() {
                    first_byte = Some(start.elapsed());
                }
                raw.extend_from_slice(&buf[..n]);
            }
            Err(e) if is_clean_eof(&e) => break,
            Err(e) => return Err(format!("failed reading response: {e}")),
        }
    }
    let total = start.elapsed();
    let starttransfer = first_byte.unwrap_or(total);

    let (response, body) = parse_response(&raw, &remote, &local)?;
    let transfer_secs = (total.saturating_sub(starttransfer)).as_secs_f64();

    Ok(HttpResult {
        final_url: url.to_string(),
        timings: Timings::from_durations(namelookup, connect, pretransfer, starttransfer, total),
        response,
        download_bytes: body.len(),
        body,
        transfer_secs,
    })
}

fn build_request(url: &Url, host: &str, opts: &RequestOptions) -> Vec<u8> {
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    if path.is_empty() {
        path.push('/');
    }

    let mut req = format!("{} {} HTTP/1.1\r\n", opts.method, path);
    req.push_str(&format!("Host: {host}\r\n"));
    req.push_str(&format!("User-Agent: {}\r\n", opts.user_agent));
    req.push_str("Accept: */*\r\n");
    req.push_str("Accept-Encoding: identity\r\n");
    req.push_str("Connection: close\r\n");
    if let Some(body) = &opts.body {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (k, v) in &opts.headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");

    let mut bytes = req.into_bytes();
    if let Some(body) = &opts.body {
        bytes.extend_from_slice(body);
    }
    bytes
}

fn parse_response(
    raw: &[u8],
    remote: &SocketAddr,
    local: &SocketAddr,
) -> Result<(ResponseInfo, Vec<u8>), String> {
    let split = find_header_end(raw)
        .ok_or_else(|| "malformed response: no header terminator".to_string())?;
    let header_bytes = &raw[..split];
    let body = raw[split + 4..].to_vec();

    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().unwrap_or("").trim().to_string();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let info = ResponseInfo {
        status_line,
        status_code,
        headers,
        remote_ip: remote.ip().to_string(),
        remote_port: remote.port().to_string(),
        local_ip: local.ip().to_string(),
        local_port: local.port().to_string(),
    };
    Ok((info, body))
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn is_redirect(code: u16) -> bool {
    matches!(code, 301 | 302 | 303 | 307 | 308)
}

/// Many servers close the connection without a TLS `close_notify`; treat that
/// (and read timeouts hit while draining) as the end of the response rather
/// than an error.
fn is_clean_eof(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Accept bare hosts like the original tool (`example.com` → `http://example.com`).
fn normalize_url(input: &str) -> Result<Url, String> {
    let candidate = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    Url::parse(&candidate).map_err(|e| format!("invalid URL \"{input}\": {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_headers() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nServer: nginx\r\n\r\nhello";
        let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let remote: SocketAddr = "93.184.216.34:80".parse().unwrap();
        let (info, body) = parse_response(raw, &remote, &local).unwrap();
        assert_eq!(info.status_code, 200);
        assert_eq!(info.status_line, "HTTP/1.1 200 OK");
        assert_eq!(body, b"hello");
        assert_eq!(
            header_value(&info.headers, "content-type").as_deref(),
            Some("text/html")
        );
        assert_eq!(info.remote_ip, "93.184.216.34");
    }

    #[test]
    fn normalizes_bare_host() {
        assert_eq!(normalize_url("example.com").unwrap().scheme(), "http");
        assert_eq!(
            normalize_url("https://example.com").unwrap().scheme(),
            "https"
        );
    }
}
