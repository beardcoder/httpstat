//! Stable v1 JSON schema, byte-for-byte compatible with the original tool's
//! `--format json` / `jsonl` output.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::http::HttpResult;
use crate::slo::Violation;
use crate::timing::TotalStats;

#[derive(Serialize)]
struct JsonResult {
    schema_version: u32,
    url: String,
    ok: bool,
    exit_code: i32,
    runs: u32,
    response: JsonResponse,
    timings_ms: JsonTimings,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_stats_ms: Option<JsonTotalStats>,
    speed: JsonSpeed,
    slo: Option<JsonSlo>,
}

#[derive(Serialize)]
struct JsonTotalStats {
    min: i64,
    mean: i64,
    max: i64,
}

#[derive(Serialize)]
struct JsonResponse {
    status_line: String,
    status_code: u16,
    remote_ip: String,
    remote_port: String,
    headers: Map<String, Value>,
}

#[derive(Serialize)]
struct JsonTimings {
    dns: i64,
    connect: i64,
    tls: i64,
    server: i64,
    transfer: i64,
    total: i64,
    namelookup: i64,
    initial_connect: i64,
    pretransfer: i64,
    starttransfer: i64,
}

#[derive(Serialize)]
struct JsonSpeed {
    download_kbs: f64,
    upload_kbs: f64,
}

#[derive(Serialize)]
struct JsonSlo {
    pass: bool,
    violations: Vec<JsonViolation>,
}

#[derive(Serialize)]
struct JsonViolation {
    key: String,
    threshold_ms: i64,
    actual_ms: i64,
}

/// Build and serialize the result. `slo` is `Some` only when the user requested
/// SLO checking (an empty violation list then means it passed). `pretty`
/// switches between 2-space indented JSON and single-line JSONL.
#[allow(clippy::too_many_arguments)]
pub fn render(
    result: &HttpResult,
    slo: Option<&[Violation]>,
    exit_code: i32,
    download_kbs: f64,
    upload_kbs: f64,
    pretty: bool,
    runs: u32,
    stats: Option<&TotalStats>,
) -> String {
    let ranges = result.timings.ranges();
    let t = &result.timings;

    let mut headers = Map::new();
    for (k, v) in &result.response.headers {
        headers.insert(k.clone(), Value::String(v.clone()));
    }

    let json = JsonResult {
        schema_version: 1,
        url: result.final_url.clone(),
        ok: exit_code == 0,
        exit_code,
        runs,
        response: JsonResponse {
            status_line: result.response.status_line.clone(),
            status_code: result.response.status_code,
            remote_ip: result.response.remote_ip.clone(),
            remote_port: result.response.remote_port.clone(),
            headers,
        },
        timings_ms: JsonTimings {
            dns: ranges.dns,
            connect: ranges.connection,
            tls: ranges.ssl,
            server: ranges.server,
            transfer: ranges.transfer,
            total: t.total_ms,
            namelookup: t.namelookup_ms,
            initial_connect: t.connect_ms,
            pretransfer: t.pretransfer_ms,
            starttransfer: t.starttransfer_ms,
        },
        total_stats_ms: stats.map(|s| JsonTotalStats {
            min: s.min_ms,
            mean: s.mean_ms,
            max: s.max_ms,
        }),
        speed: JsonSpeed {
            download_kbs,
            upload_kbs,
        },
        slo: slo.map(|violations| JsonSlo {
            pass: violations.is_empty(),
            violations: violations
                .iter()
                .map(|v| JsonViolation {
                    key: v.key.clone(),
                    threshold_ms: v.threshold_ms,
                    actual_ms: v.actual_ms,
                })
                .collect(),
        }),
    };

    if pretty {
        serde_json::to_string_pretty(&json).expect("serialization cannot fail")
    } else {
        serde_json::to_string(&json).expect("serialization cannot fail")
    }
}
