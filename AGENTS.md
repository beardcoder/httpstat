# httpstat

Native Rust CLI that visualizes HTTP request timings (DNS, TCP, TLS, server
processing, content transfer), with structured JSON output and SLO threshold
checks. A port of the original Python `reorx/httpstat`.

## Architecture

The crate is a library plus a thin binary, so every layer can be tested without
a terminal or a network.

- `src/main.rs` — the binary: parse args, lock stdout, map errors to exit codes.
- `src/lib.rs` — crate root and module map.
- `src/app.rs` — orchestration: resolve options, issue the runs, aggregate,
  render. Parameterized over the output sink and the environment.
- `src/cli.rs` — clap args, `HTTPSTAT_*` env vars (`EnvOptions`), header/method
  parsing, `RequestOptions` construction.
- `src/error.rs` — the `Error` taxonomy (`Usage` / `Request` / `Io`) that decides
  the exit code. Add a variant here rather than returning bare strings.
- `src/http/` — the HTTP/1.1 client:
  - `mod.rs` — `fetch`, the redirect loop, connection setup, phase timing.
  - `uri.rs` — URL normalization, redirect resolution, origin comparison.
  - `request.rs` — request serialization and header/method validation.
  - `response.rs` — status-line and header parsing.
  - `body.rs` — framing (`Content-Length`, chunked, until-close) and the
    incremental chunked decoder.
  - `deadline.rs` — the `--max-time` wall-clock budget.
  - `headers.rs` — ordered, case-insensitive header list.
  - `tls.rs` — rustls config, cached per trust setting; `--cacert` support.
- `src/timing.rs` — phase durations, derived milestones, `--count` aggregation
  (`Timings::mean`, `TotalStats` for min/p50/mean/p95/max).
- `src/slo.rs` — `--slo key=value` parsing and violation checks.
- `src/color.rs` — ANSI coloring, honors `NO_COLOR`.
- `src/output/` — `Report` (the render input), `pretty` (terminal layout),
  `json` (`schema_version = 1`).
- `src/testing.rs` — `#[cfg(test)]` fixtures shared by the unit tests.

## Conventions

- `cargo fmt`, and clippy is warnings-as-errors
  (`cargo clippy --all-targets -- -D warnings`). `unsafe_code` is forbidden in
  the manifest.
- Fallible functions return `crate::error::Result`. A wrong flag or env value is
  `Error::Usage`; anything that goes wrong on the wire is `Error::Request`.
- Unit tests live next to the code in `#[cfg(test)]` modules; integration tests
  are in `tests/`. Tests must not reach the public network — `tests/support`
  provides a loopback server that replays canned replies and records requests.
- Renderers write to an `impl Write` rather than calling `println!`, which is
  what makes the output assertable (and keeps a closed pipe from panicking).
- Exit codes: `0` ok, `1` request error, `2` usage error, `4` SLO violation.
- The JSON schema is additive within a `schema_version`; a rename or removal
  bumps it, and `output/json.rs` is the single place that defines the shape.

## Workflow

- `make check` — fmt, clippy and tests, the same gate CI runs.
- `make test` / `make fmt` / `make clippy` — the individual steps.
- `make build` (host) / `make build-all` (all release targets into `dist/`).
- CI (`.github/workflows/ci.yml`) runs fmt, clippy, tests and an MSRV build on
  push/PR.
- Releases: bump `version` in `Cargo.toml`, add a `CHANGELOG.md` entry, push a
  `v*` tag → `.github/workflows/release.yml` builds the four platform binaries
  with checksums and publishes a GitHub release.
