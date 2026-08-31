//! End-to-end tests of the HTTP client against a loopback server.
//!
//! These cover the parts that unit tests cannot: real sockets, real framing over
//! a real stream, and what happens when a peer misbehaves.

mod support;

use std::time::Duration;

use httpstat::error::EXIT_REQUEST;
use httpstat::http::{self, RequestOptions};
use support::{MockServer, Reply};

fn get(url: &str) -> httpstat::error::Result<http::HttpResult> {
    http::fetch(url, &RequestOptions::default())
}

fn body_of(result: &http::HttpResult) -> String {
    String::from_utf8_lossy(&result.body.bytes).to_string()
}

#[test]
fn performs_a_content_length_framed_get() {
    let server = MockServer::always(Reply::ok("hello world"));
    let result = get(&server.url("/hello")).unwrap();

    assert_eq!(result.status_code(), 200);
    assert_eq!(result.head.status_line, "HTTP/1.1 200 OK");
    assert_eq!(result.head.headers.get("content-type"), Some("text/plain"));
    assert_eq!(body_of(&result), "hello world");
    assert_eq!(result.download_bytes, 11);
    assert!(!result.https);
    assert_eq!(result.remote.port(), server.addr().port());

    let request = server.only_request();
    assert_eq!(request.request_line(), "GET /hello HTTP/1.1");
    assert_eq!(
        request.header("host").as_deref(),
        Some(server.addr().to_string().as_str())
    );
    assert_eq!(request.header("connection").as_deref(), Some("close"));
    assert!(request
        .header("user-agent")
        .unwrap()
        .starts_with("httpstat-rs/"));
}

#[test]
fn decodes_a_chunked_response_into_its_payload() {
    // The framing markers must not end up in the body, and the reported size
    // must be the payload size rather than the wire size.
    let server = MockServer::always(Reply::chunked(&["hello", " ", "world"]));
    let result = get(&server.url("/chunked")).unwrap();

    assert_eq!(body_of(&result), "hello world");
    assert_eq!(result.download_bytes, 11);
    assert!(!body_of(&result).contains('\r'));
}

#[test]
fn reads_a_body_that_is_delimited_only_by_the_connection_closing() {
    let server = MockServer::always(Reply::raw(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nno length here",
    ));
    let result = get(&server.url("/")).unwrap();
    assert_eq!(body_of(&result), "no length here");
}

#[test]
fn a_head_request_has_no_body_even_when_one_is_announced() {
    let server = MockServer::always(Reply::raw("HTTP/1.1 200 OK\r\nContent-Length: 128\r\n\r\n"));
    let opts = RequestOptions {
        method: "HEAD".into(),
        ..RequestOptions::default()
    };
    let result = http::fetch(&server.url("/"), &opts).unwrap();
    assert_eq!(result.status_code(), 200);
    assert_eq!(result.download_bytes, 0);
    assert_eq!(server.only_request().method(), "HEAD");
}

#[test]
fn a_204_has_no_body_to_wait_for() {
    let server = MockServer::always(Reply::raw("HTTP/1.1 204 No Content\r\n\r\n"));
    let result = get(&server.url("/")).unwrap();
    assert_eq!(result.status_code(), 204);
    assert_eq!(result.download_bytes, 0);
}

#[test]
fn an_interim_response_is_skipped_in_favour_of_the_final_one() {
    let server = MockServer::always(Reply::raw(
        "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok",
    ));
    let result = get(&server.url("/")).unwrap();
    assert_eq!(result.status_code(), 201);
    assert_eq!(body_of(&result), "ok");
}

#[test]
fn posts_a_body_with_the_headers_it_was_given() {
    let server = MockServer::always(Reply::ok("stored"));
    let opts = RequestOptions {
        method: "POST".into(),
        body: Some(b"{\"a\":1}".to_vec()),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())]
            .into_iter()
            .collect(),
        ..RequestOptions::default()
    };
    let result = http::fetch(&server.url("/items"), &opts).unwrap();
    assert_eq!(result.upload_bytes, 7);

    let request = server.only_request();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.header("content-length").as_deref(), Some("7"));
    assert_eq!(
        request.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(request.body_text(), "{\"a\":1}");
}

#[test]
fn a_caller_supplied_header_replaces_the_default_rather_than_duplicating_it() {
    let server = MockServer::always(Reply::ok("ok"));
    let opts = RequestOptions {
        headers: vec![("User-Agent".to_string(), "mine/1.0".to_string())]
            .into_iter()
            .collect(),
        ..RequestOptions::default()
    };
    http::fetch(&server.url("/"), &opts).unwrap();
    assert_eq!(
        server.only_request().header_values("user-agent"),
        ["mine/1.0"]
    );
}

#[test]
fn redirects_are_not_followed_unless_asked_for() {
    let server = MockServer::start(vec![Reply::redirect(302, "/final"), Reply::ok("final")]);
    let result = get(&server.url("/start")).unwrap();
    assert_eq!(result.status_code(), 302);
    assert!(result.hops.is_empty());
    assert_eq!(server.served(), 1);
}

#[test]
fn a_redirect_chain_is_followed_and_recorded() {
    let server = MockServer::start(vec![
        Reply::redirect(301, "/second"),
        Reply::redirect(302, "/third"),
        Reply::ok("arrived"),
    ]);
    let opts = RequestOptions {
        follow_redirects: true,
        ..RequestOptions::default()
    };
    let result = http::fetch(&server.url("/first"), &opts).unwrap();

    assert_eq!(result.status_code(), 200);
    assert_eq!(body_of(&result), "arrived");
    assert_eq!(result.final_url, server.url("/third"));
    assert_eq!(result.hops.len(), 2);
    assert_eq!(result.hops[0].status_code, 301);
    assert_eq!(result.hops[1].status_code, 302);

    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| r.path().to_string())
        .collect();
    assert_eq!(paths, ["/first", "/second", "/third"]);
}

#[test]
fn a_303_continues_with_a_bodyless_get() {
    let server = MockServer::start(vec![Reply::redirect(303, "/result"), Reply::ok("done")]);
    let opts = RequestOptions {
        method: "POST".into(),
        body: Some(b"payload".to_vec()),
        follow_redirects: true,
        ..RequestOptions::default()
    };
    http::fetch(&server.url("/submit"), &opts).unwrap();

    let requests = server.requests();
    assert_eq!(requests[0].method(), "POST");
    assert_eq!(requests[1].method(), "GET");
    assert!(requests[1].body.is_empty());
    assert_eq!(requests[1].header("content-length"), None);
}

#[test]
fn a_307_replays_the_method_and_body() {
    let server = MockServer::start(vec![Reply::redirect(307, "/again"), Reply::ok("done")]);
    let opts = RequestOptions {
        method: "POST".into(),
        body: Some(b"payload".to_vec()),
        follow_redirects: true,
        ..RequestOptions::default()
    };
    http::fetch(&server.url("/submit"), &opts).unwrap();

    let requests = server.requests();
    assert_eq!(requests[1].method(), "POST");
    assert_eq!(requests[1].body_text(), "payload");
}

#[test]
fn a_redirect_loop_stops_at_the_limit() {
    let server = MockServer::always(Reply::redirect(302, "/loop"));
    let opts = RequestOptions {
        follow_redirects: true,
        max_redirects: 3,
        ..RequestOptions::default()
    };
    let error = http::fetch(&server.url("/loop"), &opts).unwrap_err();
    assert!(error.to_string().contains("too many redirects"), "{error}");
    assert_eq!(
        server.served(),
        4,
        "one request per hop, then the limit stops it"
    );
}

#[test]
fn credentials_are_not_replayed_across_origins() {
    // The redirect points at a host we never connect to; what matters is the
    // request the first server saw and what the client was prepared to send on.
    let server = MockServer::start(vec![Reply::redirect(302, "http://127.0.0.1:1/next")]);
    let opts = RequestOptions {
        follow_redirects: true,
        headers: vec![("Authorization".to_string(), "Bearer secret".to_string())]
            .into_iter()
            .collect(),
        ..RequestOptions::default()
    };
    let error = http::fetch(&server.url("/start"), &opts).unwrap_err();
    assert!(error.to_string().contains("connect"), "{error}");
    assert_eq!(
        server.only_request().header("authorization").as_deref(),
        Some("Bearer secret"),
        "the original origin still gets its credentials"
    );
}

#[test]
fn a_truncated_content_length_body_is_an_error_rather_than_a_short_read() {
    let server = MockServer::always(Reply::Truncated(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nonly-this".to_vec(),
    ));
    let error = get(&server.url("/")).unwrap_err();
    assert!(
        error.to_string().contains("closed after 9 of 100"),
        "{error}"
    );
    assert_eq!(error.exit_code(), EXIT_REQUEST);
}

#[test]
fn a_truncated_chunked_body_is_an_error() {
    let server = MockServer::always(Reply::Truncated(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n".to_vec(),
    ));
    let error = get(&server.url("/")).unwrap_err();
    assert!(error.to_string().contains("final chunk"), "{error}");
}

#[test]
fn a_response_that_is_not_http_is_rejected() {
    let server = MockServer::always(Reply::raw("SSH-2.0-OpenSSH_9.0\r\n\r\n"));
    let error = get(&server.url("/")).unwrap_err();
    assert!(
        error.to_string().contains("expected an HTTP status line"),
        "{error}"
    );
}

#[test]
fn a_server_that_closes_without_replying_is_reported_clearly() {
    let server = MockServer::always(Reply::HangUp);
    let error = get(&server.url("/")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("closed the connection without sending a response"),
        "{error}"
    );
}

#[test]
fn a_connection_refused_names_the_address() {
    // Port 1 on loopback is reserved and never listening.
    let error = get("http://127.0.0.1:1/").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not connect to 127.0.0.1:1"),
        "{error}"
    );
    assert_eq!(error.exit_code(), EXIT_REQUEST);
}

#[test]
fn max_time_is_a_budget_for_the_whole_request() {
    let server = MockServer::always(Reply::Delayed(
        Duration::from_secs(10),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    ));
    let opts = RequestOptions {
        max_time: Some(Duration::from_millis(150)),
        ..RequestOptions::default()
    };
    let started = std::time::Instant::now();
    let error = http::fetch(&server.url("/slow"), &opts).unwrap_err();

    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(error.to_string().contains("--max-time"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the deadline must cut the request short, took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_body_that_stalls_mid_transfer_hits_the_deadline_instead_of_being_truncated() {
    // A read timeout that resets on every byte would let this run forever; a
    // deadline stops it, and the partial body is reported as a failure.
    let server = MockServer::always(Reply::Stalled(
        b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\nhalf".to_vec(),
        Duration::from_secs(10),
        b"-the-rest-here".to_vec(),
    ));
    let opts = RequestOptions {
        max_time: Some(Duration::from_millis(200)),
        ..RequestOptions::default()
    };
    let error = http::fetch(&server.url("/stall"), &opts).unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
}

#[test]
fn timings_are_ordered_and_cover_the_whole_request() {
    let server = MockServer::always(Reply::ok("hello"));
    let result = get(&server.url("/")).unwrap();
    let t = result.timings;

    assert!(t.namelookup_ms <= t.connect_ms, "{t:?}");
    assert!(t.connect_ms <= t.pretransfer_ms, "{t:?}");
    assert!(t.pretransfer_ms <= t.starttransfer_ms, "{t:?}");
    assert!(t.starttransfer_ms <= t.total_ms, "{t:?}");

    // A plain HTTP request does no TLS handshake, so that phase is empty.
    assert_eq!(t.pretransfer_ms, t.connect_ms);
    assert_eq!(result.timings.ranges().ssl, 0);
}

#[test]
fn an_invalid_url_fails_before_any_socket_is_opened() {
    for url in ["ftp://example.com", "http://", "   "] {
        let error = get(url).unwrap_err();
        assert_eq!(error.exit_code(), httpstat::error::EXIT_USAGE, "{url}");
    }
}
