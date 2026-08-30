//! The structured output schema (`--format json` / `jsonl`).
//!
//! `schema_version` is the contract with anything parsing this output. Fields
//! are only ever added within a version; a rename or a removal bumps it.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::output::Report;

/// The current schema version. Bump on any breaking change to the shape below.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct JsonResult<'a> {
    schema_version: u32,
    url: &'a str,
    method: &'a str,
    ok: bool,
    exit_code: i32,
    runs: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    redirects: Vec<JsonHop<'a>>,
    response: JsonResponse<'a>,
    timings_ms: JsonTimings,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_stats_ms: Option<JsonTotalStats>,
    speed: JsonSpeed,
    slo: Option<JsonSlo<'a>>,
}

#[derive(Serialize)]
struct JsonHop<'a> {
    url: &'a str,
    status_code: u16,
}

#[derive(Serialize)]
struct JsonTotalStats {
    min: i64,
    p50: i64,
    mean: i64,
    p95: i64,
    max: i64,
}

#[derive(Serialize)]
struct JsonResponse<'a> {
    status_line: &'a str,
    status_code: u16,
    http_version: &'a str,
    remote_ip: String,
    remote_port: String,
    /// Decoded payload size in bytes, counting anything not kept in memory.
    body_bytes: usize,
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
struct JsonSlo<'a> {
    pass: bool,
    violations: Vec<JsonViolation<'a>>,
}

#[derive(Serialize)]
struct JsonViolation<'a> {
    key: &'a str,
    threshold_ms: i64,
    actual_ms: i64,
}

/// Serialize a report. `indented` switches between 2-space JSON and one-line JSONL.
pub fn render(report: &Report<'_>, indented: bool) -> String {
    let result = report.result;
    let ranges = result.timings.ranges();
    let t = &result.timings;

    let json = JsonResult {
        schema_version: SCHEMA_VERSION,
        url: &result.final_url,
        method: &result.method,
        ok: report.ok(),
        exit_code: report.exit_code as i32,
        runs: report.runs,
        redirects: result
            .hops
            .iter()
            .map(|hop| JsonHop {
                url: &hop.url,
                status_code: hop.status_code,
            })
            .collect(),
        response: JsonResponse {
            status_line: &result.head.status_line,
            status_code: result.head.status_code,
            http_version: &result.head.version,
            remote_ip: result.remote.ip().to_string(),
            remote_port: result.remote.port().to_string(),
            body_bytes: result.download_bytes,
            headers: headers_object(report),
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
        total_stats_ms: report.stats.map(|s| JsonTotalStats {
            min: s.min_ms,
            p50: s.p50_ms,
            mean: s.mean_ms,
            p95: s.p95_ms,
            max: s.max_ms,
        }),
        speed: JsonSpeed {
            download_kbs: report.download_kbs,
            upload_kbs: report.upload_kbs,
        },
        slo: report.slo_requested.then(|| JsonSlo {
            pass: report.violations.is_empty(),
            violations: report
                .violations
                .iter()
                .map(|v| JsonViolation {
                    key: &v.key,
                    threshold_ms: v.threshold_ms,
                    actual_ms: v.actual_ms,
                })
                .collect(),
        }),
    };

    let serialized = if indented {
        serde_json::to_string_pretty(&json)
    } else {
        serde_json::to_string(&json)
    };
    serialized.expect("the report is composed of plain serializable data")
}

/// Response headers as a JSON object.
///
/// A field name may legally repeat (`Set-Cookie`), which an object cannot hold
/// twice — the values are joined with `, ` as RFC 9110 §5.3 prescribes, so no
/// header is silently lost.
fn headers_object(report: &Report<'_>) -> Map<String, Value> {
    let headers = &report.result.head.headers;
    let mut object = Map::new();
    for name in headers.names() {
        let joined = headers.get_all(name).collect::<Vec<_>>().join(", ");
        object.insert(name.to_string(), Value::String(joined));
    }
    object
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use serde_json::Value;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("render must emit valid JSON")
    }

    #[test]
    fn renders_the_documented_schema_for_a_simple_request() {
        let result = testing::result();
        let json = parse(&render(&testing::report(&result), true));

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["url"], "https://example.com/");
        assert_eq!(json["method"], "GET");
        assert_eq!(json["ok"], true);
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["runs"], 1);
        assert_eq!(json["response"]["status_line"], "HTTP/1.1 200 OK");
        assert_eq!(json["response"]["status_code"], 200);
        assert_eq!(json["response"]["http_version"], "1.1");
        assert_eq!(json["response"]["remote_ip"], "93.184.216.34");
        assert_eq!(json["response"]["remote_port"], "443");
        assert_eq!(json["response"]["body_bytes"], 11);
        assert_eq!(json["response"]["headers"]["Content-Type"], "text/plain");
        assert_eq!(json["slo"], Value::Null);
        // Absent sections stay absent rather than turning into empty arrays.
        assert!(json.get("redirects").is_none());
        assert!(json.get("total_stats_ms").is_none());
    }

    #[test]
    fn timings_expose_both_phase_durations_and_cumulative_milestones() {
        let result = testing::result();
        let json = parse(&render(&testing::report(&result), true));
        let t = &json["timings_ms"];
        assert_eq!(t["dns"], 5);
        assert_eq!(t["connect"], 10);
        assert_eq!(t["tls"], 15);
        assert_eq!(t["server"], 50);
        assert_eq!(t["transfer"], 20);
        assert_eq!(t["total"], 100);
        assert_eq!(t["namelookup"], 5);
        assert_eq!(t["initial_connect"], 15);
        assert_eq!(t["pretransfer"], 30);
        assert_eq!(t["starttransfer"], 80);
    }

    #[test]
    fn repeated_response_headers_are_joined_rather_than_dropped() {
        let mut result = testing::result();
        result.head = testing::head(
            "HTTP/1.1 200 OK\nSet-Cookie: a=1\nSet-Cookie: b=2\nContent-Type: text/plain",
        );
        let json = parse(&render(&testing::report(&result), true));
        assert_eq!(json["response"]["headers"]["Set-Cookie"], "a=1, b=2");
        assert_eq!(json["response"]["headers"]["Content-Type"], "text/plain");
    }

    #[test]
    fn a_passing_slo_check_is_distinguishable_from_no_check_at_all() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.slo_requested = true;
        let json = parse(&render(&report, true));
        assert_eq!(json["slo"]["pass"], true);
        assert_eq!(json["slo"]["violations"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn violations_are_listed_with_the_failing_exit_code() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.slo_requested = true;
        report.violations = vec![testing::violation("total", 50, 100)];
        report.exit_code = crate::error::EXIT_SLO;
        let json = parse(&render(&report, true));
        assert_eq!(json["ok"], false);
        assert_eq!(json["exit_code"], 4);
        assert_eq!(json["slo"]["pass"], false);
        assert_eq!(json["slo"]["violations"][0]["key"], "total");
        assert_eq!(json["slo"]["violations"][0]["threshold_ms"], 50);
        assert_eq!(json["slo"]["violations"][0]["actual_ms"], 100);
    }

    #[test]
    fn multiple_runs_add_the_total_time_distribution() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.runs = 3;
        report.stats = Some(testing::stats(&[100, 200, 300]));
        let json = parse(&render(&report, true));
        assert_eq!(json["runs"], 3);
        assert_eq!(json["total_stats_ms"]["min"], 100);
        assert_eq!(json["total_stats_ms"]["p50"], 200);
        assert_eq!(json["total_stats_ms"]["mean"], 200);
        assert_eq!(json["total_stats_ms"]["p95"], 300);
        assert_eq!(json["total_stats_ms"]["max"], 300);
    }

    #[test]
    fn a_followed_redirect_chain_is_reported() {
        let mut result = testing::result();
        result.hops = vec![
            testing::hop("http://example.com/", 301),
            testing::hop("https://example.com/", 302),
        ];
        let json = parse(&render(&testing::report(&result), true));
        assert_eq!(json["redirects"][0]["url"], "http://example.com/");
        assert_eq!(json["redirects"][0]["status_code"], 301);
        assert_eq!(json["redirects"][1]["status_code"], 302);
    }

    #[test]
    fn jsonl_is_one_line_and_json_is_indented() {
        let result = testing::result();
        let report = testing::report(&result);
        let line = render(&report, false);
        assert!(!line.contains('\n'), "jsonl must be a single line");
        let indented = render(&report, true);
        assert!(indented.contains("\n  \"url\""), "{indented}");
        // Both encode the same data.
        assert_eq!(parse(&line), parse(&indented));
    }
}
