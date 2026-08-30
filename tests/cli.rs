//! End-to-end tests of the `httpstat` binary against a loopback server.

mod support;

use std::path::PathBuf;
use std::process::{Command, Output};

use support::{MockServer, Reply};

/// Exit codes, mirrored from the crate so a change to either side is a test
/// failure rather than a silent contract break.
const EXIT_OK: i32 = 0;
const EXIT_REQUEST: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_SLO: i32 = 4;

/// The binary, with colour and temp-file writing switched off so assertions see
/// plain text and tests leave nothing behind.
fn httpstat() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_httpstat"));
    command
        .env("NO_COLOR", "1")
        .env("HTTPSTAT_SAVE_BODY", "0")
        .env_remove("HTTPSTAT_SHOW_BODY")
        .env_remove("HTTPSTAT_SHOW_IP")
        .env_remove("HTTPSTAT_SHOW_SPEED")
        .env_remove("HTTPSTAT_METRICS_ONLY")
        .env_remove("HTTPSTAT_DEBUG");
    command
}

fn run(args: &[&str]) -> Output {
    httpstat()
        .args(args)
        .output()
        .expect("the binary should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output)).expect("stdout should be valid JSON")
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("httpstat_test_{}_{name}", std::process::id()))
}

// ---------------------------------------------------------------- basics ----

#[test]
fn version_flag_prints_the_crate_version() {
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn no_arguments_prints_help_and_succeeds() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    let text = stdout(&output);
    assert!(text.contains("Usage"), "{text}");
    assert!(text.contains("--slo"), "{text}");
}

#[test]
fn the_long_help_documents_the_exit_codes_and_environment() {
    let text = stdout(&run(&["--help"]));
    for expected in [
        "Exit codes:",
        "HTTPSTAT_SHOW_BODY",
        "NO_COLOR",
        "--max-time",
        "--count",
    ] {
        assert!(
            text.contains(expected),
            "help is missing {expected}:\n{text}"
        );
    }
}

// ------------------------------------------------------------ exit codes ----

#[test]
fn an_invalid_option_value_is_a_usage_error() {
    for (args, expected) in [
        (
            vec!["--format", "xml", "http://127.0.0.1:1/"],
            "invalid format",
        ),
        (
            vec!["--slo", "bogus=10", "http://127.0.0.1:1/"],
            "unknown SLO key",
        ),
        (
            vec!["--slo", "total=0", "http://127.0.0.1:1/"],
            "must be positive",
        ),
        (
            vec!["-H", "no-colon", "http://127.0.0.1:1/"],
            "invalid header",
        ),
        (vec!["ftp://example.com"], "unsupported URL scheme"),
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(EXIT_USAGE), "{args:?}");
        assert!(
            stderr(&output).contains(expected),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn a_header_that_tries_to_inject_a_crlf_is_refused() {
    let output = httpstat()
        .args(["-H", "X-Test: a\r\nX-Evil: 1", "http://127.0.0.1:1/"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(
        stderr(&output).contains("carriage return"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_out_of_range_count_is_rejected_by_the_parser() {
    let output = run(&["--count", "0", "http://127.0.0.1:1/"]);
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
}

#[test]
fn a_failed_request_exits_with_the_request_error_code() {
    let output = run(&["http://127.0.0.1:1/"]);
    assert_eq!(output.status.code(), Some(EXIT_REQUEST));
    assert!(
        stderr(&output).contains("could not connect"),
        "{}",
        stderr(&output)
    );
}

// --------------------------------------------------------------- output ----

#[test]
fn the_pretty_report_shows_the_request_status_and_timing_box() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&[&server.url("/path")]);

    assert_eq!(output.status.code(), Some(EXIT_OK));
    let text = stdout(&output);
    assert!(
        text.contains(&format!("GET {}", server.url("/path"))),
        "{text}"
    );
    assert!(text.contains("HTTP/1.1 200 OK"), "{text}");
    assert!(text.contains("text/plain"), "{text}");
    assert!(
        text.contains("DNS Lookup   TCP Connection   Server Processing"),
        "{text}"
    );
    assert!(text.contains("total:"), "{text}");
    // Plain HTTP has no TLS column.
    assert!(!text.contains("TLS Handshake"), "{text}");
    // NO_COLOR is honoured.
    assert!(
        !text.contains('\x1b'),
        "output should carry no escape codes"
    );
}

#[test]
fn json_output_matches_the_documented_schema() {
    let server = MockServer::always(Reply::ok("hello world"));
    let output = run(&["-f", "json", &server.url("/api")]);
    assert_eq!(output.status.code(), Some(EXIT_OK));

    let json = json(&output);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["url"], server.url("/api"));
    assert_eq!(json["method"], "GET");
    assert_eq!(json["ok"], true);
    assert_eq!(json["exit_code"], 0);
    assert_eq!(json["runs"], 1);
    assert_eq!(json["response"]["status_code"], 200);
    assert_eq!(json["response"]["body_bytes"], 11);
    assert_eq!(json["response"]["remote_ip"], "127.0.0.1");
    assert_eq!(json["slo"], serde_json::Value::Null);
    assert!(json["timings_ms"]["total"].is_i64());
}

#[test]
fn jsonl_output_is_exactly_one_line() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&["-f", "jsonl", &server.url("/")]);
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "{text}");
    serde_json::from_str::<serde_json::Value>(text.trim()).expect("valid JSON");
}

#[test]
fn metrics_only_still_selects_json() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = httpstat()
        .env("HTTPSTAT_METRICS_ONLY", "1")
        .arg(server.url("/"))
        .output()
        .unwrap();
    assert_eq!(json(&output)["schema_version"], 1);
}

#[test]
fn an_invalid_environment_toggle_is_a_usage_error_that_names_it() {
    let output = httpstat()
        .env("HTTPSTAT_SHOW_BODY", "banana")
        .arg("http://127.0.0.1:1/")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(
        stderr(&output).contains("HTTPSTAT_SHOW_BODY"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn environment_toggles_control_the_optional_sections() {
    let server = MockServer::always(Reply::ok("the body text"));

    let hidden = httpstat()
        .env("HTTPSTAT_SHOW_IP", "0")
        .arg(server.url("/"))
        .output()
        .unwrap();
    assert!(!stdout(&hidden).contains("Connected to"));

    let shown = httpstat()
        .env("HTTPSTAT_SHOW_BODY", "1")
        .env("HTTPSTAT_SHOW_SPEED", "true")
        .arg(server.url("/"))
        .output()
        .unwrap();
    let text = stdout(&shown);
    assert!(text.contains("the body text"), "{text}");
    assert!(text.contains("speed_download:"), "{text}");
}

#[test]
fn a_chunked_response_body_is_shown_without_its_framing() {
    let server = MockServer::always(Reply::chunked(&["chunk one ", "chunk two"]));
    let output = httpstat()
        .env("HTTPSTAT_SHOW_BODY", "1")
        .arg(server.url("/"))
        .output()
        .unwrap();
    let text = stdout(&output);
    assert!(text.contains("chunk one chunk two"), "{text}");
    assert!(
        !text.contains("\r\n9\r\n"),
        "chunk markers leaked into the body:\n{text}"
    );
}

// ------------------------------------------------------------------ slo ----

#[test]
fn a_met_slo_threshold_passes() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&["-f", "json", "--slo", "total=60000", &server.url("/")]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    assert_eq!(json(&output)["slo"]["pass"], true);
}

#[test]
fn a_breached_slo_threshold_exits_with_code_four() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&["-f", "json", "--slo", "total=0.5,dns=1", &server.url("/")]);
    // A sub-millisecond threshold is not expressible; use an integer of 1ms
    // through the flag below instead.
    assert_eq!(output.status.code(), Some(EXIT_USAGE));

    let output = run(&["-f", "json", "--slo", "total=1", &server.url("/")]);
    if output.status.code() == Some(EXIT_SLO) {
        let json = json(&output);
        assert_eq!(json["ok"], false);
        assert_eq!(json["exit_code"], 4);
        assert_eq!(json["slo"]["pass"], false);
        assert_eq!(json["slo"]["violations"][0]["key"], "total");
    } else {
        // A loopback request can genuinely finish inside a millisecond.
        assert_eq!(output.status.code(), Some(EXIT_OK));
        assert_eq!(json(&output)["slo"]["pass"], true);
    }
}

#[test]
fn a_violation_is_reported_in_the_pretty_output_too() {
    let server = MockServer::always(Reply::Delayed(
        std::time::Duration::from_millis(60),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    ));
    let output = run(&["--slo", "total=10", &server.url("/slow")]);
    assert_eq!(output.status.code(), Some(EXIT_SLO));
    assert!(
        stdout(&output).contains("SLO VIOLATION: total ="),
        "{}",
        stdout(&output)
    );
}

// -------------------------------------------------------------- requests ----

#[test]
fn repeated_runs_are_averaged_and_summarized() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&["-f", "json", "-n", "3", &server.url("/")]);

    assert_eq!(output.status.code(), Some(EXIT_OK));
    let json = json(&output);
    assert_eq!(json["runs"], 3);
    assert!(json["total_stats_ms"]["min"].is_i64());
    assert!(json["total_stats_ms"]["p95"].is_i64());
    assert_eq!(server.served(), 3, "each run needs its own connection");
}

#[test]
fn a_failing_run_reports_which_one_failed() {
    let output = run(&["-n", "3", "http://127.0.0.1:1/"]);
    assert_eq!(output.status.code(), Some(EXIT_REQUEST));
    assert!(
        stderr(&output).contains("run 1 of 3"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_post_sends_its_data_and_headers() {
    let server = MockServer::always(Reply::ok("stored"));
    let output = run(&[
        "-f",
        "json",
        "-d",
        "{\"a\":1}",
        "-H",
        "Content-Type: application/json",
        &server.url("/items"),
    ]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    assert_eq!(json(&output)["method"], "POST");

    let request = server.only_request();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.body_text(), "{\"a\":1}");
    assert_eq!(
        request.header("content-type").as_deref(),
        Some("application/json")
    );
}

#[test]
fn redirects_are_followed_only_with_the_location_flag() {
    let server = MockServer::start(vec![Reply::redirect(302, "/final"), Reply::ok("arrived")]);

    let output = run(&["-f", "json", &server.url("/start")]);
    assert_eq!(json(&output)["response"]["status_code"], 302);

    let server = MockServer::start(vec![Reply::redirect(302, "/final"), Reply::ok("arrived")]);
    let output = run(&["-f", "json", "-L", &server.url("/start")]);
    let json = json(&output);
    assert_eq!(json["response"]["status_code"], 200);
    assert_eq!(json["redirects"][0]["status_code"], 302);
    assert_eq!(json["url"], server.url("/final"));
}

#[test]
fn max_redirects_needs_the_location_flag() {
    let output = run(&["--max-redirects", "2", "http://127.0.0.1:1/"]);
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(
        stderr(&output).contains("--location"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn max_time_cuts_a_slow_request_short() {
    let server = MockServer::always(Reply::Delayed(
        std::time::Duration::from_secs(10),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    ));
    let started = std::time::Instant::now();
    let output = run(&["--max-time", "0.2", &server.url("/slow")]);

    assert_eq!(output.status.code(), Some(EXIT_REQUEST));
    assert!(stderr(&output).contains("timed out"), "{}", stderr(&output));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

// ----------------------------------------------------------------- save ----

#[test]
fn save_writes_json_alongside_the_pretty_report() {
    let server = MockServer::always(Reply::ok("hello"));
    let path = temp_path("save_pretty.json");
    let _ = std::fs::remove_file(&path);

    let output = run(&["--save", path.to_str().unwrap(), &server.url("/")]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    assert!(
        stdout(&output).contains("DNS Lookup"),
        "the terminal report is unaffected"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["schema_version"], 1);
    assert_eq!(saved["response"]["status_code"], 200);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_unwritable_save_path_is_reported() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&[
        "--save",
        "/nonexistent-directory/out.json",
        &server.url("/"),
    ]);
    assert_eq!(output.status.code(), Some(EXIT_REQUEST));
    assert!(
        stderr(&output).contains("could not write"),
        "{}",
        stderr(&output)
    );
}

// ------------------------------------------------------------- plumbing ----

#[test]
#[cfg(unix)]
fn a_reader_that_closes_the_pipe_early_is_not_an_error() {
    let server = MockServer::always(Reply::ok("hello"));
    let script = format!(
        "{} -f json {} | head -c 1",
        env!("CARGO_BIN_EXE_httpstat"),
        server.url("/")
    );
    let output = Command::new("sh")
        .args(["-c", &format!("set -e; {script}")])
        .env("NO_COLOR", "1")
        .env("HTTPSTAT_SAVE_BODY", "0")
        .output()
        .expect("sh should run");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("panicked"),
        "a closed pipe must not panic: {}",
        stderr(&output)
    );
}

#[test]
fn a_bare_host_and_port_is_treated_as_a_url() {
    let server = MockServer::always(Reply::ok("hello"));
    let output = run(&["-f", "json", &format!("{}/bare", server.addr())]);
    assert_eq!(output.status.code(), Some(EXIT_OK));
    assert_eq!(json(&output)["response"]["status_code"], 200);
    assert_eq!(server.only_request().path(), "/bare");
}
