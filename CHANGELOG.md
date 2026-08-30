# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [3.0.0]

A correctness and robustness pass over the whole tool, plus the test suite to
keep it that way.

### Breaking

- **Exit codes now distinguish usage errors from request failures.** An invalid
  flag, header, `--slo` spec or `HTTPSTAT_*` value exits with `2`; a request that
  fails to complete exits with `1`. Previously both exited with `1`. This is the
  contract the project documented but did not implement. Scripts that treated any
  non-zero exit as "request failed" are unaffected; scripts that matched on `1`
  specifically should be checked.
- **The minimum supported Rust version is now 1.86**, which is what the locked
  dependency set has required in practice (`clap` needs 1.85, and the `icu`
  crates that `url` pulls in need 1.86).

### Fixed

- **Chunked responses are decoded.** `Transfer-Encoding: chunked` bodies were
  previously reported with their chunk-size framing still embedded, corrupting
  the displayed body, the saved body and the byte count. Most servers use chunked
  encoding, so this affected most requests.
- **A truncated response is an error, not a short read.** A connection that dies
  mid-body, or a `--max-time` that expires mid-transfer, used to be reported as a
  successful request with a silently incomplete body.
- **`--max-time` is a wall-clock budget.** It was installed as a socket read
  timeout that reset on every read, so a slow trickle of bytes could keep a
  request alive indefinitely. It now covers the whole request, redirects
  included, and `--connect-timeout` can no longer be multiplied by the number of
  resolved addresses.
- **`HTTPSTAT_SHOW_BODY` no longer panics** on a body whose 1024th byte falls in
  the middle of a multi-byte character.
- **Header injection through `-H` and `--user-agent` is rejected.** A CR or LF in
  a header value used to be written straight into the request, splicing in
  arbitrary headers.
- **Caller headers no longer duplicate the defaults.** `-H 'User-Agent: x'` sent
  two `User-Agent` headers; it now replaces the default.
- **Piping into a short reader no longer panics.** `httpstat … | head` exits
  quietly with `0`.
- **Repeated response headers are no longer dropped** from JSON output; values
  for a repeated name (`Set-Cookie`) are joined as RFC 9110 prescribes.
- **The timing box header row is aligned again.** A Rust line-continuation escape
  was eating its two-space indent.
- **Saved body files are created privately** (mode `0600` on Unix) with a name
  that cannot collide, and never overwrite an existing file.
- **Malformed responses are reported.** A reply that is not HTTP, has no status
  code, or has a broken header line used to be reported as status `0`.

### Added

- `--cacert <PATH>` and `$SSL_CERT_FILE` to verify TLS against a private PEM
  bundle, for internal PKI and TLS-inspecting proxies.
- `--max-redirects <N>` to bound a redirect chain (default 10).
- `p50` and `p95` of the total time under `--count`, in both the pretty summary
  and `total_stats_ms`.
- JSON additions within `schema_version` 1: `method`, `response.http_version`,
  `response.body_bytes`, and a `redirects` array describing a followed chain.
- The redirect chain is shown above the status line in pretty output.
- `HTTP/1.1 1xx` interim responses are skipped rather than reported as the result.
- Response bodies are streamed with an 8 MiB retention cap, so measuring a large
  download cannot exhaust memory while still counting every byte.

### Changed

- Redirects follow RFC 9110 method semantics: `303` (and `301`/`302` from a
  non-GET) continues as a bodyless `GET`, while `307`/`308` replay the method and
  body.
- `Authorization`, `Cookie` and `Proxy-Authorization` are dropped when a redirect
  crosses to a different origin.
- The TLS configuration is built once per trust setting instead of once per
  request, so `--count` no longer measures root-store parsing as handshake time.
- `NO_COLOR` follows the specification: it disables color when set to a
  *non-empty* value.
- Error messages name the phase that failed and, for timeouts, the flag that
  caused it.

### Internal

- The crate is now a library plus a thin binary, split into focused modules
  (`app`, `error`, `http::{body,deadline,headers,request,response,tls,uri}`,
  `output::Report`).
- Renderers write to an `impl Write` instead of calling `println!`, which makes
  the terminal output directly assertable.
- 177 tests, up from 16: unit tests beside every module and integration tests that
  drive both the library and the binary against a loopback HTTP server with no
  network dependency.

## [2.2.1]

- Documentation: macOS install instructions for Apple Silicon and Intel.

## [2.2.0]

- Removed the proportional phase bar from the pretty output.
- Dimmed Unicode vertical strokes in the timing box.

## [2.1.0]

- `--count` for averaged timings over repeated runs, with a progress bar.

## [2.0.1]

- Fall back to other resolved addresses on connect failure.

## [2.0.0]

- Rewritten in native Rust with cross-platform binaries; the Python
  implementation and its runtime dependency on `curl` are gone.

[3.0.0]: https://github.com/beardcoder/httpstat/releases/tag/v3.0.0
[2.2.1]: https://github.com/beardcoder/httpstat/releases/tag/v2.2.1
[2.2.0]: https://github.com/beardcoder/httpstat/releases/tag/v2.2.0
[2.1.0]: https://github.com/beardcoder/httpstat/releases/tag/v2.1.0
[2.0.1]: https://github.com/beardcoder/httpstat/releases/tag/v2.0.1
