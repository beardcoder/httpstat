//! Typed errors and the exit-code contract.
//!
//! Every failure the tool can produce falls into one of three buckets, and the
//! bucket alone determines the process exit code — see [`Error::exit_code`].
//! Scripts can therefore tell "you invoked me wrongly" (2) apart from "the
//! request did not complete" (1) apart from "the response was too slow" (4).

use std::fmt;
use std::io;

/// Everything went fine.
pub const EXIT_OK: u8 = 0;
/// The request could not be completed (DNS, TCP, TLS, protocol, timeout, I/O).
pub const EXIT_REQUEST: u8 = 1;
/// The command line or environment asked for something invalid.
pub const EXIT_USAGE: u8 = 2;
/// The request succeeded but violated at least one `--slo` threshold.
pub const EXIT_SLO: u8 = 4;

/// A fatal error, carrying a message that is already end-user readable.
#[derive(Debug)]
pub enum Error {
    /// Invalid flag value, header, SLO spec or `HTTPSTAT_*` variable.
    Usage(String),
    /// The request itself failed: resolution, connection, TLS, framing, timeout.
    Request(String),
    /// A local I/O failure, such as writing the `--save` file.
    Io(String, io::Error),
}

impl Error {
    pub fn usage(message: impl Into<String>) -> Self {
        Error::Usage(message.into())
    }

    pub fn request(message: impl Into<String>) -> Self {
        Error::Request(message.into())
    }

    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io(context.into(), source)
    }

    /// Whether this is a downstream reader closing the pipe (`… | head`),
    /// which is normal and not a failure of ours.
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Error::Io(_, source) if source.kind() == io::ErrorKind::BrokenPipe)
    }

    /// The process exit code this error maps to.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Usage(_) => EXIT_USAGE,
            Error::Request(_) | Error::Io(..) => EXIT_REQUEST,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(m) | Error::Request(m) => f.write_str(m),
            Error::Io(context, source) => write!(f, "{context}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(_, source) => Some(source),
            _ => None,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_error_kind() {
        assert_eq!(Error::usage("bad flag").exit_code(), EXIT_USAGE);
        assert_eq!(Error::request("dns failed").exit_code(), EXIT_REQUEST);
        assert_eq!(
            Error::io("write", io::Error::other("disk full")).exit_code(),
            EXIT_REQUEST
        );
    }

    #[test]
    fn a_closed_pipe_is_recognised() {
        let broken = Error::io("write", io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        assert!(broken.is_broken_pipe());
        assert!(!Error::request("nope").is_broken_pipe());
        assert!(!Error::io("write", io::Error::other("disk full")).is_broken_pipe());
    }

    #[test]
    fn io_errors_render_with_their_context_and_source() {
        let err = Error::io("could not write out.json", io::Error::other("disk full"));
        assert_eq!(err.to_string(), "could not write out.json: disk full");
        assert!(std::error::Error::source(&err).is_some());
    }
}
