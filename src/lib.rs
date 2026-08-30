//! httpstat — see where the time in an HTTP request actually goes.
//!
//! The crate is split so that each concern is testable on its own:
//!
//! - [`cli`] — argument and `HTTPSTAT_*` environment parsing.
//! - [`app`] — orchestration: resolve options, issue the runs, render.
//! - [`http`] — the HTTP/1.1 client that times every connection phase by hand.
//! - [`timing`] — phase arithmetic and the `--count` distribution.
//! - [`slo`] — `--slo` threshold parsing and checking.
//! - [`output`] — the JSON schema and the terminal visualization.
//! - [`error`] — the error taxonomy that decides the process exit code.
//!
//! The binary in `main.rs` is a thin wrapper around [`app::run`].

pub mod app;
pub mod cli;
pub mod color;
pub mod error;
pub mod http;
pub mod output;
pub mod slo;
pub mod timing;

#[cfg(test)]
pub mod testing;

/// The default `User-Agent`, e.g. `httpstat-rs/2.3.0`.
pub const USER_AGENT: &str = concat!("httpstat-rs/", env!("CARGO_PKG_VERSION"));

/// The crate version, as published.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
