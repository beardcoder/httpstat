//! Response body framing and decoding.
//!
//! The previous implementation always read until the connection closed and
//! handed the raw bytes on as the body. That is wrong for the two framings the
//! majority of servers actually use: a `Content-Length` body is complete before
//! EOF, and a `Transfer-Encoding: chunked` body carries size markers that are
//! not part of the payload. This module implements RFC 9112 §6 framing so the
//! body we report, display and save is the real payload, and so a connection
//! that dies mid-body is reported as an error instead of a short read.

use crate::error::{Error, Result};
use crate::http::headers::Headers;

/// Longest chunk-size / trailer line we will buffer, a guard against a peer
/// that sends an unbounded line to exhaust memory.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// How the payload of a response is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// The response cannot have a body (HEAD, 1xx, 204, 304).
    Empty,
    /// Exactly this many bytes follow the header block.
    Length(u64),
    /// `Transfer-Encoding: chunked`.
    Chunked,
    /// No delimiter: the body ends when the peer closes the connection.
    UntilClose,
}

/// Decide how a response body is delimited, per RFC 9112 §6.3.
pub fn framing_for(method: &str, status: u16, headers: &Headers) -> Result<Framing> {
    // A HEAD response describes the body it *would* have sent but never sends
    // one; 1xx/204/304 never carry a body either.
    if method.eq_ignore_ascii_case("HEAD") || matches!(status, 100..=199 | 204 | 304) {
        return Ok(Framing::Empty);
    }
    if let Some(encoding) = headers.get("transfer-encoding") {
        // Only the *final* coding decides framing; anything else means the
        // sender must close the connection to delimit the body.
        let last = encoding.rsplit(',').next().unwrap_or("").trim();
        return Ok(if last.eq_ignore_ascii_case("chunked") {
            Framing::Chunked
        } else {
            Framing::UntilClose
        });
    }
    match content_length(headers)? {
        Some(len) => Ok(Framing::Length(len)),
        None => Ok(Framing::UntilClose),
    }
}

/// Parse `Content-Length`, rejecting garbage and conflicting repeats.
fn content_length(headers: &Headers) -> Result<Option<u64>> {
    let mut found: Option<u64> = None;
    for raw in headers.get_all("content-length") {
        // A single field line may itself be a list ("42, 42") after proxies
        // merged repeated headers; every element must agree.
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let value: u64 = part.parse().map_err(|_| {
                Error::request(format!(
                    "malformed response: invalid Content-Length \"{raw}\""
                ))
            })?;
            match found {
                Some(existing) if existing != value => {
                    return Err(Error::request(
                        "malformed response: conflicting Content-Length headers".to_string(),
                    ))
                }
                _ => found = Some(value),
            }
        }
    }
    Ok(found)
}

/// The payload of a response, plus what had to be dropped to bound memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// The decoded payload, capped at the reader's retention limit.
    pub bytes: Vec<u8>,
    /// Total decoded payload size, counting bytes dropped past the limit.
    pub total: usize,
}

impl Body {
    pub fn empty() -> Self {
        Body {
            bytes: Vec::new(),
            total: 0,
        }
    }

    /// Whether bytes were dropped because the retention limit was reached.
    pub fn truncated(&self) -> bool {
        self.total > self.bytes.len()
    }
}

/// Incrementally consumes response bytes according to a [`Framing`].
///
/// The reader retains at most `retain_limit` bytes so an enormous download
/// cannot exhaust memory, while still counting every byte for the transfer-rate
/// figures.
#[derive(Debug)]
pub struct BodyReader {
    framing: Framing,
    chunked: ChunkedDecoder,
    bytes: Vec<u8>,
    total: usize,
    retain_limit: usize,
    done: bool,
}

impl BodyReader {
    pub fn new(framing: Framing, retain_limit: usize) -> Self {
        BodyReader {
            framing,
            chunked: ChunkedDecoder::new(),
            bytes: Vec::new(),
            total: 0,
            retain_limit,
            done: matches!(framing, Framing::Empty | Framing::Length(0)),
        }
    }

    /// Feed freshly read bytes. Returns `true` once the body is complete and no
    /// further reads are needed.
    pub fn push(&mut self, input: &[u8]) -> Result<bool> {
        if self.done || input.is_empty() {
            return Ok(self.done);
        }
        match self.framing {
            Framing::Empty => {}
            Framing::Length(len) => {
                let remaining = len.saturating_sub(self.total as u64);
                let take = remaining.min(input.len() as u64) as usize;
                self.retain(&input[..take]);
                if self.total as u64 >= len {
                    self.done = true;
                }
            }
            Framing::UntilClose => self.retain(input),
            Framing::Chunked => {
                let mut decoded = Vec::new();
                self.done = self.chunked.push(input, &mut decoded)?;
                self.retain(&decoded);
            }
        }
        Ok(self.done)
    }

    /// Signal that the peer closed the connection, and take the body.
    ///
    /// Errors when the framing promised more data than arrived, which is a
    /// truncated response rather than a successful one.
    pub fn finish_at_eof(self) -> Result<Body> {
        match self.framing {
            Framing::Length(len) if (self.total as u64) < len => Err(Error::request(format!(
                "connection closed after {} of {len} announced body bytes",
                self.total
            ))),
            Framing::Chunked if !self.done => Err(Error::request(
                "connection closed before the final chunk of a chunked response".to_string(),
            )),
            _ => Ok(self.into_body()),
        }
    }

    /// Take the body once [`push`](Self::push) reported completion.
    pub fn finish(self) -> Body {
        self.into_body()
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    fn into_body(self) -> Body {
        Body {
            bytes: self.bytes,
            total: self.total,
        }
    }

    fn retain(&mut self, data: &[u8]) {
        self.total += data.len();
        let room = self.retain_limit.saturating_sub(self.bytes.len());
        if room > 0 {
            self.bytes.extend_from_slice(&data[..room.min(data.len())]);
        }
    }
}

/// State machine for `Transfer-Encoding: chunked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    /// Reading a chunk-size line (possibly with extensions).
    Size,
    /// Reading `n` more payload bytes of the current chunk.
    Data(u64),
    /// Reading the CRLF that terminates a chunk's payload.
    DataEnd,
    /// Reading trailer fields after the terminating zero-size chunk.
    Trailer,
    /// The terminating chunk and trailers have been consumed.
    Done,
}

/// A byte-at-a-time chunked-transfer decoder.
#[derive(Debug)]
struct ChunkedDecoder {
    state: ChunkState,
    line: Vec<u8>,
}

impl ChunkedDecoder {
    fn new() -> Self {
        ChunkedDecoder {
            state: ChunkState::Size,
            line: Vec::new(),
        }
    }

    /// Decode `input`, appending payload bytes to `out`. Returns `true` once the
    /// terminating chunk and its trailers have been seen.
    fn push(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<bool> {
        let mut cursor = 0usize;
        while cursor < input.len() {
            match self.state {
                ChunkState::Done => break,
                ChunkState::Data(remaining) => {
                    let take = (remaining as usize).min(input.len() - cursor);
                    out.extend_from_slice(&input[cursor..cursor + take]);
                    cursor += take;
                    let left = remaining - take as u64;
                    self.state = if left == 0 {
                        ChunkState::DataEnd
                    } else {
                        ChunkState::Data(left)
                    };
                }
                ChunkState::DataEnd => {
                    // Consume the CRLF that follows chunk data, tolerating a
                    // bare LF from sloppy servers.
                    match input[cursor] {
                        b'\r' => cursor += 1,
                        b'\n' => {
                            cursor += 1;
                            self.state = ChunkState::Size;
                        }
                        other => {
                            return Err(Error::request(format!(
                                "malformed chunked response: expected CRLF after chunk data, got byte {other:#04x}"
                            )))
                        }
                    }
                }
                ChunkState::Size | ChunkState::Trailer => {
                    let Some(line) = self.read_line(input, &mut cursor)? else {
                        break;
                    };
                    if self.state == ChunkState::Size {
                        let size = parse_chunk_size(&line)?;
                        self.state = if size == 0 {
                            ChunkState::Trailer
                        } else {
                            ChunkState::Data(size)
                        };
                    } else if line.trim().is_empty() {
                        // The blank line closes the trailer section.
                        self.state = ChunkState::Done;
                    }
                }
            }
        }
        Ok(self.state == ChunkState::Done)
    }

    /// Accumulate bytes until a newline, returning the completed line without
    /// its terminator. `None` means the line is still incomplete.
    fn read_line(&mut self, input: &[u8], cursor: &mut usize) -> Result<Option<String>> {
        while *cursor < input.len() {
            let byte = input[*cursor];
            *cursor += 1;
            if byte == b'\n' {
                let line = String::from_utf8_lossy(&self.line)
                    .trim_end_matches('\r')
                    .to_string();
                self.line.clear();
                return Ok(Some(line));
            }
            if self.line.len() >= MAX_LINE_BYTES {
                return Err(Error::request(
                    "malformed chunked response: chunk header exceeds 8 KiB".to_string(),
                ));
            }
            self.line.push(byte);
        }
        Ok(None)
    }
}

/// Parse a chunk-size line: hex digits, optionally followed by `;extensions`.
fn parse_chunk_size(line: &str) -> Result<u64> {
    let digits = line.split(';').next().unwrap_or("").trim();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::request(format!(
            "malformed chunked response: invalid chunk size \"{line}\""
        )));
    }
    u64::from_str_radix(digits, 16).map_err(|_| {
        Error::request(format!(
            "malformed chunked response: chunk size \"{digits}\" is out of range"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 1024 * 1024;

    fn headers(pairs: &[(&str, &str)]) -> Headers {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn decode_chunked(parts: &[&[u8]]) -> Result<Body> {
        let mut reader = BodyReader::new(Framing::Chunked, LIMIT);
        for part in parts {
            if reader.push(part)? {
                return Ok(reader.finish());
            }
        }
        reader.finish_at_eof()
    }

    #[test]
    fn head_and_bodyless_statuses_have_no_body() {
        let h = headers(&[("Content-Length", "42")]);
        assert_eq!(framing_for("HEAD", 200, &h).unwrap(), Framing::Empty);
        assert_eq!(framing_for("head", 200, &h).unwrap(), Framing::Empty);
        for status in [100, 199, 204, 304] {
            assert_eq!(framing_for("GET", status, &h).unwrap(), Framing::Empty);
        }
    }

    #[test]
    fn transfer_encoding_wins_over_content_length() {
        let h = headers(&[("Transfer-Encoding", "chunked"), ("Content-Length", "42")]);
        assert_eq!(framing_for("GET", 200, &h).unwrap(), Framing::Chunked);

        let h = headers(&[("Transfer-Encoding", "gzip, chunked")]);
        assert_eq!(framing_for("GET", 200, &h).unwrap(), Framing::Chunked);

        // A non-chunked final coding cannot delimit the body.
        let h = headers(&[("Transfer-Encoding", "gzip")]);
        assert_eq!(framing_for("GET", 200, &h).unwrap(), Framing::UntilClose);
    }

    #[test]
    fn content_length_framing_and_its_error_cases() {
        let h = headers(&[("content-length", "7")]);
        assert_eq!(framing_for("GET", 200, &h).unwrap(), Framing::Length(7));

        let h = headers(&[("Content-Length", "7"), ("content-length", "7")]);
        assert_eq!(framing_for("GET", 200, &h).unwrap(), Framing::Length(7));

        let h = headers(&[("Content-Length", "7"), ("Content-Length", "9")]);
        assert!(framing_for("GET", 200, &h).is_err());

        let h = headers(&[("Content-Length", "seven")]);
        assert!(framing_for("GET", 200, &h).is_err());

        assert_eq!(
            framing_for("GET", 200, &Headers::new()).unwrap(),
            Framing::UntilClose
        );
    }

    #[test]
    fn length_framing_stops_exactly_at_the_announced_size() {
        let mut reader = BodyReader::new(Framing::Length(5), LIMIT);
        assert!(!reader.push(b"abc").unwrap());
        // The trailing bytes belong to nothing and must not leak into the body.
        assert!(reader.push(b"de-overshoot").unwrap());
        let body = reader.finish();
        assert_eq!(body.bytes, b"abcde");
        assert_eq!(body.total, 5);
    }

    #[test]
    fn a_short_length_framed_body_is_an_error_not_a_short_read() {
        let mut reader = BodyReader::new(Framing::Length(10), LIMIT);
        reader.push(b"abc").unwrap();
        let err = reader.finish_at_eof().unwrap_err();
        assert!(err.to_string().contains("closed after 3 of 10"), "{err}");
    }

    #[test]
    fn zero_length_and_empty_framings_complete_immediately() {
        assert!(BodyReader::new(Framing::Length(0), LIMIT).is_done());
        assert!(BodyReader::new(Framing::Empty, LIMIT).is_done());
        assert_eq!(
            BodyReader::new(Framing::Empty, LIMIT).finish(),
            Body::empty()
        );
    }

    #[test]
    fn until_close_keeps_everything_until_eof() {
        let mut reader = BodyReader::new(Framing::UntilClose, LIMIT);
        assert!(!reader.push(b"hello ").unwrap());
        assert!(!reader.push(b"world").unwrap());
        assert_eq!(reader.finish_at_eof().unwrap().bytes, b"hello world");
    }

    #[test]
    fn chunked_decoding_strips_the_framing() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let body = decode_chunked(&[raw]).unwrap();
        assert_eq!(body.bytes, b"hello world");
        assert_eq!(body.total, 11);
    }

    #[test]
    fn chunked_decoding_survives_arbitrary_packet_boundaries() {
        let raw: &[u8] = b"4\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\n\r\n";
        let expected = b"Wikipedia in\r\n\r\nchunks.";
        // Feeding one byte at a time exercises every state transition.
        let singles: Vec<&[u8]> = raw.chunks(1).collect();
        assert_eq!(decode_chunked(&singles).unwrap().bytes, expected);
        for split in [1, 5, 9, 17, 30] {
            let (a, b) = raw.split_at(split);
            assert_eq!(
                decode_chunked(&[a, b]).unwrap().bytes,
                expected,
                "split at {split}"
            );
        }
    }

    #[test]
    fn chunked_decoding_handles_extensions_uppercase_hex_and_trailers() {
        let raw = b"A;name=value\r\n0123456789\r\n0\r\nX-Trailer: yes\r\nX-More: 1\r\n\r\n";
        assert_eq!(decode_chunked(&[raw]).unwrap().bytes, b"0123456789");
    }

    #[test]
    fn chunked_decoding_rejects_malformed_input() {
        assert!(decode_chunked(&[b"zz\r\nhello\r\n0\r\n\r\n"]).is_err());
        assert!(decode_chunked(&[b"5\r\nhelloXX0\r\n\r\n"]).is_err());
        // A stream that stops before the terminating chunk is truncated.
        let err = decode_chunked(&[b"5\r\nhello\r\n"]).unwrap_err();
        assert!(err.to_string().contains("final chunk"), "{err}");
    }

    #[test]
    fn oversized_bodies_are_counted_but_only_partly_retained() {
        let mut reader = BodyReader::new(Framing::UntilClose, 4);
        reader.push(b"0123456789").unwrap();
        let body = reader.finish_at_eof().unwrap();
        assert_eq!(body.bytes, b"0123");
        assert_eq!(body.total, 10);
        assert!(body.truncated());
    }

    #[test]
    fn an_overlong_chunk_header_is_rejected() {
        let flood = vec![b'0'; MAX_LINE_BYTES + 16];
        assert!(decode_chunked(&[&flood]).is_err());
    }
}
