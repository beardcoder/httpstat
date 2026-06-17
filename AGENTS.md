# httpstat

Native Rust CLI that visualizes HTTP request timings (DNS, TCP, TLS, server
processing, content transfer), with structured JSON output and SLO threshold
checks. A port of the original Python `reorx/httpstat`.

## Architecture

- `src/main.rs` — entry point: option resolution, orchestration, exit codes.
- `src/cli.rs` — clap arg parsing, `HTTPSTAT_*` env vars, `parse_bool`, method/header helpers.
- `src/http.rs` — in-process HTTP/1.1 client measuring each connection phase by
  hand (DNS, TCP connect, TLS handshake, TTFB, transfer). TLS via rustls + ring;
  no curl, no OpenSSL. Tries every resolved address so an unreachable AAAA falls
  back to IPv4.
- `src/timing.rs` — phase durations and derived milestones.
- `src/slo.rs` — `--slo key=value` parsing and violation checks (keys: total,
  connect, ttfb, dns, tls).
- `src/color.rs` — ANSI coloring, honors `NO_COLOR`.
- `src/output/` — `pretty` (terminal layout), `json` / `jsonl` (schema_version=1).

## Conventions

- `cargo fmt`, and clippy is warnings-as-errors (`cargo clippy --all-targets -- -D warnings`).
- Unit tests live next to the code in `#[cfg(test)]` modules; CLI integration
  tests are in `tests/cli.rs`. Tests must not depend on the network.
- Exit codes: `0` ok, `1` request error, `2` usage error, `4` SLO violation.

## Workflow

- `make test` / `make fmt` / `make clippy` — local checks.
- `make build` (host) / `make build-all` (all release targets into `dist/`).
- CI (`.github/workflows/ci.yml`) runs fmt + clippy + tests on push/PR.
- Releases: push a `v*` tag → `.github/workflows/release.yml` builds the four
  platform binaries and publishes a GitHub release. Bump `version` in `Cargo.toml`.
