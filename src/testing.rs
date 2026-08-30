//! Fixtures shared by the unit tests.
//!
//! Only compiled for `cargo test`; building a realistic [`HttpResult`] by hand
//! in every renderer test would bury what each test is actually asserting.

use std::net::SocketAddr;

use crate::http::body::Body;
use crate::http::response::parse_head;
use crate::http::{Head, Hop, HttpResult};
use crate::output::Report;
use crate::slo::Violation;
use crate::timing::{Timings, TotalStats};

pub fn head(raw: &str) -> Head {
    parse_head(raw.replace('\n', "\r\n").as_bytes()).expect("valid response head")
}

pub fn timings() -> Timings {
    Timings {
        namelookup_ms: 5,
        connect_ms: 15,
        pretransfer_ms: 30,
        starttransfer_ms: 80,
        total_ms: 100,
    }
}

/// A successful HTTPS GET of `https://example.com/` returning `hello world`.
pub fn result() -> HttpResult {
    let body = b"hello world".to_vec();
    HttpResult {
        final_url: "https://example.com/".to_string(),
        https: true,
        method: "GET".to_string(),
        head: head("HTTP/1.1 200 OK\nContent-Type: text/plain\nServer: nginx"),
        remote: "93.184.216.34:443".parse::<SocketAddr>().unwrap(),
        local: "192.168.1.5:54321".parse::<SocketAddr>().unwrap(),
        timings: timings(),
        download_bytes: body.len(),
        upload_bytes: 0,
        body: Body {
            total: body.len(),
            bytes: body,
        },
        transfer_secs: 0.02,
        hops: Vec::new(),
    }
}

/// A report over [`result`] with no SLO checking and a single run.
pub fn report(result: &HttpResult) -> Report<'_> {
    Report {
        request_line: format!("{} {}", result.method, result.final_url),
        download_kbs: crate::output::kbs(result.download_bytes, result.transfer_secs),
        upload_kbs: 0.0,
        violations: Vec::new(),
        slo_requested: false,
        runs: 1,
        stats: None,
        exit_code: 0,
        result,
    }
}

pub fn violation(key: &str, threshold_ms: i64, actual_ms: i64) -> Violation {
    Violation {
        key: key.to_string(),
        threshold_ms,
        actual_ms,
    }
}

pub fn hop(url: &str, status_code: u16) -> Hop {
    Hop {
        url: url.to_string(),
        status_code,
    }
}

pub fn stats(totals: &[i64]) -> TotalStats {
    let samples: Vec<Timings> = totals
        .iter()
        .map(|ms| Timings {
            total_ms: *ms,
            ..timings()
        })
        .collect();
    TotalStats::from_samples(&samples).expect("at least one sample")
}
