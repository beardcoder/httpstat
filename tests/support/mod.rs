//! A loopback HTTP server for the integration tests.
//!
//! The tests must not depend on the network, but they do need a real socket to
//! exercise DNS resolution, connection setup, response framing and timeouts end
//! to end. This server binds `127.0.0.1:0`, replays canned raw responses and
//! records what it was sent, so every assertion is deterministic.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// What the server does with one accepted connection.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Send these exact bytes, then close.
    Raw(Vec<u8>),
    /// Wait, then send these bytes and close. Used to trip `--max-time`.
    Delayed(Duration, Vec<u8>),
    /// Send a prefix, wait, then send the rest. Used to test a stalled body.
    Stalled(Vec<u8>, Duration, Vec<u8>),
    /// Send a prefix and close mid-response.
    Truncated(Vec<u8>),
    /// Close without sending anything.
    HangUp,
}

impl Reply {
    /// `200 OK` with a `Content-Length`-framed body.
    pub fn ok(body: &str) -> Reply {
        Reply::status(200, "OK", body)
    }

    pub fn status(code: u16, reason: &str, body: &str) -> Reply {
        Reply::Raw(
            format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
    }

    /// A redirect to `location`.
    pub fn redirect(code: u16, location: &str) -> Reply {
        Reply::Raw(
            format!("HTTP/1.1 {code} Moved\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        )
    }

    /// A chunked response whose payload is the concatenation of `chunks`.
    pub fn chunked(chunks: &[&str]) -> Reply {
        let mut raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n"
                .to_vec();
        for chunk in chunks {
            raw.extend_from_slice(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes());
        }
        raw.extend_from_slice(b"0\r\n\r\n");
        Reply::Raw(raw)
    }

    pub fn raw(text: &str) -> Reply {
        Reply::Raw(text.as_bytes().to_vec())
    }
}

/// A request as the server received it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub head: String,
    pub body: Vec<u8>,
}

impl Recorded {
    /// The request line, e.g. `GET /path HTTP/1.1`.
    pub fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or_default()
    }

    pub fn method(&self) -> &str {
        self.request_line().split(' ').next().unwrap_or_default()
    }

    pub fn path(&self) -> &str {
        self.request_line().split(' ').nth(1).unwrap_or_default()
    }

    /// Every value sent for `name`, in order, matched case-insensitively.
    pub fn header_values(&self, name: &str) -> Vec<String> {
        self.head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
            .collect()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.header_values(name).into_iter().next()
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// A single-threaded HTTP server serving canned replies on loopback.
pub struct MockServer {
    addr: SocketAddr,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    served: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Serve `replies` in order; once exhausted the last one repeats, so a
    /// `--count N` test needs only one reply.
    pub fn start(replies: Vec<Reply>) -> MockServer {
        assert!(
            !replies.is_empty(),
            "a mock server needs at least one reply"
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let served = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let handle = thread::spawn({
            let recorded = Arc::clone(&recorded);
            let served = Arc::clone(&served);
            let shutdown = Arc::clone(&shutdown);
            move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { break };
                    // The connection the Drop impl opens to wake this loop
                    // sends nothing; it is not a request and must not consume a
                    // reply or be recorded.
                    let Some(request) = read_request(&mut stream) else {
                        continue;
                    };
                    // Recording before replying is what makes the tests
                    // deterministic: once the client can see a response byte,
                    // the request behind it is already visible to the test.
                    recorded.lock().expect("recording lock").push(request);
                    let index = served.fetch_add(1, Ordering::SeqCst);
                    respond(
                        stream,
                        replies[index.min(replies.len() - 1)].clone(),
                        &shutdown,
                    );
                }
            }
        });

        MockServer {
            addr,
            recorded,
            served,
            shutdown,
            handle: Some(handle),
        }
    }

    /// A server that answers every request with the same reply.
    pub fn always(reply: Reply) -> MockServer {
        MockServer::start(vec![reply])
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// An absolute URL for `path`, which must start with `/`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// How many requests have been answered.
    pub fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }

    /// The requests received so far, in order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().expect("recording lock").clone()
    }

    /// The single request received, failing the test if there was not exactly one.
    pub fn only_request(&self) -> Recorded {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "expected exactly one request");
        requests.into_iter().next().expect("one request")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Unblock the accept loop so the thread can observe the shutdown flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Perform one reply on an already-read connection.
fn respond(mut stream: TcpStream, reply: Reply, shutdown: &AtomicBool) {
    match reply {
        Reply::Raw(bytes) => {
            let _ = stream.write_all(&bytes);
        }
        Reply::Delayed(delay, bytes) => {
            sleep_interruptibly(delay, shutdown);
            let _ = stream.write_all(&bytes);
        }
        Reply::Stalled(head, delay, rest) => {
            let _ = stream.write_all(&head);
            let _ = stream.flush();
            sleep_interruptibly(delay, shutdown);
            let _ = stream.write_all(&rest);
        }
        Reply::Truncated(bytes) => {
            let _ = stream.write_all(&bytes);
        }
        Reply::HangUp => {}
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// Sleep in short steps so a test that is already finished does not have to
/// wait out the full delay when the server is dropped.
fn sleep_interruptibly(total: Duration, shutdown: &AtomicBool) {
    const STEP: Duration = Duration::from_millis(20);
    let mut slept = Duration::ZERO;
    while slept < total && !shutdown.load(Ordering::SeqCst) {
        thread::sleep(STEP.min(total - slept));
        slept += STEP;
    }
}

/// Read a request head and, if one is announced, its `Content-Length` body.
fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut raw: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 4096];
    let split = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return None,
            Ok(n) => raw.extend_from_slice(&buffer[..n]),
            Err(_) => return None,
        }
    };

    let head = String::from_utf8_lossy(&raw[..split]).replace("\r\n", "\n");
    let mut body = raw[split + 4..].to_vec();
    let length: usize = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    while body.len() < length {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&buffer[..n]),
        }
    }
    Some(Recorded { head, body })
}
