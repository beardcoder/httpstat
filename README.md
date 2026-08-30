# httpstat

![screenshot](screenshot.png)

**Where did the time in that request actually go?**

`httpstat` answers that question. It issues one HTTP request and shows you the
breakdown as a labelled timeline — how long DNS took, how long the TCP handshake
took, how long TLS took, how long the server thought about it, and how long the
bytes took to arrive. When a page feels slow, this is what tells you *which part*
is slow, in one line of terminal.

It is a **single static binary 🌟** written in **native Rust 🦀** with **no
runtime dependency 👏**: the request is performed in-process by a small HTTP/1.1
client, so each phase is timed at the moment it happens rather than inferred
afterwards. There is no `curl` binary to shell out to and no OpenSSL to link —
TLS is pure Rust (rustls + ring), so the Linux builds are fully static.

```bash
httpstat https://example.com
```

## Why you might want it

- **Diagnose a slow endpoint in one command.** A 900 ms request is a very
  different problem depending on whether 850 ms of it was DNS, the TLS handshake,
  or the server.
- **Put a number on it, repeatedly.** `--count` issues the request N times and
  reports the mean of every phase along with the min / p50 / p95 / max spread, so
  you can tell a slow service from a noisy network.
- **Fail a pipeline on latency.** `--slo total=500,ttfb=200` exits with code `4`
  when a threshold is breached, which is all a CI step or an alerting cron needs.
- **Feed a dashboard.** `--format json` emits a stable, versioned schema; `jsonl`
  emits one line per run for log pipelines.

## Features

- **Beautiful terminal output** — the timing breakdown of DNS, TCP, TLS, server
  processing and content transfer, in the layout the original `httpstat` made familiar
- **Correct HTTP framing** — `Content-Length` and `Transfer-Encoding: chunked`
  are decoded properly, so the body you see and save is the real payload and a
  connection that dies mid-response is reported as an error, not a short read
- **Structured output** — `--format json` / `jsonl` with a documented,
  versioned schema (`schema_version`)
- **SLO thresholds** — `--slo total=500,connect=100`, exit code `4` on violation
- **Repeat and aggregate** — `--count 20` with mean phases and a min/p50/p95/max
  spread of the total
- **Real timeouts** — `--max-time` is a wall-clock budget for the whole request,
  redirects included, not a per-read timeout that resets on every byte
- **Safe redirects** — `-L` follows them, downgrades `POST` to `GET` on 303 and
  drops `Authorization` and `Cookie` when a redirect crosses to another origin
- **Custom trust** — `--cacert` (or `$SSL_CERT_FILE`) for private PKI and
  TLS-inspecting proxies; `-k` when you truly do not care
- **NO_COLOR support** — respects the [NO_COLOR](https://no-color.org) convention

## Installation

### Download a prebuilt binary

Grab the archive for your platform from the
[latest release](https://github.com/beardcoder/httpstat/releases/latest),
extract it, and put the `httpstat` binary on your `PATH`:

**macOS (Apple Silicon):**

```bash
curl -fsSL -o httpstat.tar.gz \
  https://github.com/beardcoder/httpstat/releases/latest/download/httpstat-aarch64-apple-darwin.tar.gz
tar -xzf httpstat.tar.gz
sudo install httpstat /usr/local/bin/
```

**macOS (Intel):**

```bash
curl -fsSL -o httpstat.tar.gz \
  https://github.com/beardcoder/httpstat/releases/latest/download/httpstat-x86_64-apple-darwin.tar.gz
tar -xzf httpstat.tar.gz
sudo install httpstat /usr/local/bin/
```

**Linux x86_64:**

```bash
curl -fsSL -o httpstat.tar.gz \
  https://github.com/beardcoder/httpstat/releases/latest/download/httpstat-x86_64-unknown-linux-musl.tar.gz
tar -xzf httpstat.tar.gz
sudo install httpstat /usr/local/bin/
```

Each archive is published with a `.sha256` checksum file next to it. Available
targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`x86_64-apple-darwin`, `aarch64-apple-darwin`.

### Install with cargo

```bash
cargo install --git https://github.com/beardcoder/httpstat
```

### Build from source

```bash
git clone https://github.com/beardcoder/httpstat
cd httpstat
cargo build --release
# binary at target/release/httpstat
```

Building requires Rust 1.85 or newer.

## Usage

```bash
httpstat httpbin.org/get
```

A bare host is accepted and defaults to `http://`; so is a `host:port` pair. The
request is issued in-process, so only the documented options below are supported
(not arbitrary curl flags). Run `httpstat --help` for the full list.

| Option | Description |
| --- | --- |
| `-f, --format <FORMAT>` | Output format: `pretty` (default), `json`, `jsonl` |
| `-n, --count <N>` | Repeat the request N times and report averaged timings |
| `--slo <SPEC>` | SLO thresholds, e.g. `total=500,connect=100` (exit 4 on violation) |
| `--save <PATH>` | Save the structured JSON result to a file |
| `-X, --request <METHOD>` | HTTP method (defaults to GET, or POST when `--data` is given) |
| `-H, --header <HEADER>` | Extra request header `Name: Value` (repeatable) |
| `-d, --data <DATA>` | Request body data |
| `-L, --location` | Follow HTTP redirects |
| `--max-redirects <N>` | Maximum redirects to follow (default 10, needs `-L`) |
| `-k, --insecure` | Skip TLS certificate verification |
| `--cacert <PATH>` | Verify TLS against this PEM bundle instead of the built-in roots |
| `-A, --user-agent <UA>` | User-Agent header value |
| `--connect-timeout <SECONDS>` | Maximum time allowed for the TCP connection |
| `--max-time <SECONDS>` | Maximum total time for the request, redirects included |

```bash
httpstat httpbin.org/post -X POST -d '{"a":"b"}' -H 'Content-Type: application/json' -L
```

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The request completed, and any `--slo` thresholds held |
| `1` | The request failed (DNS, connection, TLS, protocol, timeout, local I/O) |
| `2` | The command line or environment was invalid |
| `4` | The request completed but breached an `--slo` threshold |

A failing request and a failing threshold are deliberately different codes: a
monitoring job usually wants to page on one and open a ticket on the other.

### Averaging over multiple runs

Pass `-n/--count` to issue the request several times and report the mean of each
timing phase — handy for smoothing out jitter on a noisy connection. Each run
opens a fresh connection, so DNS, TCP and TLS are measured every time.

```bash
httpstat -n 10 https://example.com
```

A live progress bar tracks the runs (shown only on an interactive terminal, so
piped output stays clean). The timing box then shows the averaged milestones,
followed by the distribution of the total time:

```
averaged over 10 runs — total min 234ms · p50 290ms · mean 301ms · p95 380ms · max 401ms
```

The median and the p95 are what separate "this service is slow" from "this
service is usually fine and occasionally terrible" — a mean alone hides that.

In JSON output the run count is reported as `runs`, and `total_stats_ms`
(`{min, p50, mean, p95, max}`) is added whenever `runs > 1`.

### Structured output

Use `--format` (`-f`) to get machine-readable output:

```bash
httpstat httpbin.org/get --format json
```

```json
{
  "schema_version": 1,
  "url": "https://httpbin.org/get",
  "method": "GET",
  "ok": true,
  "exit_code": 0,
  "runs": 1,
  "response": {
    "status_line": "HTTP/1.1 200 OK",
    "status_code": 200,
    "http_version": "1.1",
    "remote_ip": "...",
    "remote_port": "443",
    "body_bytes": 312,
    "headers": {"content-type": "application/json", "server": "nginx"}
  },
  "timings_ms": {
    "dns": 5, "connect": 10, "tls": 15,
    "server": 50, "transfer": 20, "total": 100,
    "namelookup": 5, "initial_connect": 15,
    "pretransfer": 30, "starttransfer": 80
  },
  "speed": { "download_kbs": 1234.5, "upload_kbs": 0.0 },
  "slo": null
}
```

`timings_ms` reports both views of the same measurements: `dns`, `connect`,
`tls`, `server` and `transfer` are the duration *of each phase*, while
`namelookup`, `initial_connect`, `pretransfer` and `starttransfer` are the
cumulative milestones from the start of the request (the same values curl's
`time_*` variables report).

Two sections appear only when they apply: `redirects` lists the chain when `-L`
followed one, and `total_stats_ms` appears when `--count` was above 1. Fields are
only ever added within a `schema_version`; a rename or a removal bumps it.

Use `--format jsonl` for compact single-line JSON (useful for log pipelines).

### SLO thresholds

Check response times against thresholds. Exits with code `4` on violation:

```bash
httpstat httpbin.org/get --slo total=500,connect=100,ttfb=200
```

| Key | Compares against |
| --- | --- |
| `total` | The complete request |
| `connect` | DNS plus TCP connect |
| `ttfb` | Time to first byte |
| `dns` | DNS resolution |
| `tls` | Everything up to the first request byte |

All thresholds are whole milliseconds. In pretty mode, violations are printed in
red at the end of the output; in JSON mode they appear in the `slo` field:

```json
{
  "slo": {
    "pass": false,
    "violations": [
      { "key": "total", "threshold_ms": 500, "actual_ms": 823 }
    ]
  }
}
```

`"slo": null` means no check was requested — distinct from `"pass": true`, which
means the check ran and held.

Combined with `--count`, the thresholds are checked against the *averaged*
timings, which is usually what you want in a cron job:

```bash
httpstat https://api.example.com/health -n 5 --slo ttfb=250 || alert
```

### Save results

Write structured JSON output to a file, whatever `--format` is set to:

```bash
httpstat httpbin.org/get --save result.json           # pretty on screen, JSON on disk
httpstat httpbin.org/get --format json --save result.json
```

### TLS

Certificates are verified against a root store compiled into the binary, so
there is nothing to install and nothing to keep in sync. Two escape hatches
exist for environments that need one:

```bash
httpstat https://internal.corp --cacert /etc/ssl/corp-root.pem   # private PKI
export SSL_CERT_FILE=/etc/ssl/corp-root.pem                      # same, via the environment
httpstat https://self-signed.test -k                             # verify nothing
```

`--cacert` and `$SSL_CERT_FILE` *replace* the built-in roots rather than adding
to them, the same way `curl --cacert` does. `-k` disables verification entirely
and should be reserved for hosts you already trust by other means.

### Environment variables

Run `httpstat --help` to see the full explanation. All booleans accept
`1/true/yes/on` and `0/false/no/off`; anything else is a usage error rather than
a silent default.

| Variable | Default | Effect |
| --- | --- | --- |
| `HTTPSTAT_SHOW_BODY` | `false` | Show the response body (first 1024 bytes) |
| `HTTPSTAT_SHOW_IP` | `true` | Show remote/local IP and port |
| `HTTPSTAT_SHOW_SPEED` | `false` | Show download/upload speed |
| `HTTPSTAT_SAVE_BODY` | `true` | Store the body in a temporary file |
| `HTTPSTAT_METRICS_ONLY` | `false` | Equivalent to `--format json` (kept for compatibility) |
| `HTTPSTAT_DEBUG` | `false` | Print the resolved options to stderr |
| `SSL_CERT_FILE` | unset | PEM bundle to verify TLS against |
| `NO_COLOR` | unset | When set to a non-empty value, disables colored output ([no-color.org](https://no-color.org)) |

For convenience, export these in your `.zshrc` or `.bashrc`:

```bash
export HTTPSTAT_SHOW_IP=false
export HTTPSTAT_SHOW_SPEED=true
export HTTPSTAT_SAVE_BODY=false
```

## Limitations

Worth knowing before you reach for it:

- **HTTP/1.1 only.** ALPN advertises `http/1.1`, so a server that also speaks
  HTTP/2 will answer in HTTP/1.1. HTTP/2 and HTTP/3 timings are not measured.
- **No proxy support.** `HTTP_PROXY` / `HTTPS_PROXY` are ignored; the connection
  goes straight to the host being measured, which is usually what you want when
  measuring it.
- **No compression.** The request asks for `Accept-Encoding: identity`, so the
  transfer time is for the uncompressed payload.
- **The connection is never reused.** Every run measures a cold connection —
  that is the point of the tool, but it means the numbers are not what a
  keep-alive client would see.
- **Bodies are counted in full but only the first 8 MiB are kept** for display
  and `--save`, so measuring a large download cannot exhaust memory.

## Development

```bash
make test       # cargo test
make fmt        # cargo fmt
make clippy     # cargo clippy -D warnings
make check      # fmt --check, clippy and tests, the same gate CI runs
make build      # release build for the host
make build-all  # cross-compile all release targets into dist/
```

The test suite has no network dependency: the integration tests start a
loopback HTTP server (`tests/support/mod.rs`) that replays canned responses and
records what it was sent, so framing, redirects, timeouts and exit codes are all
covered deterministically.

CI runs fmt, clippy, the tests and an MSRV check on every push and pull request;
pushing a `v*` tag builds the cross-platform binaries and publishes a GitHub
release. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Credits

A native Rust port of the original Python
[reorx/httpstat](https://github.com/reorx/httpstat) by Reorx, whose terminal
layout this keeps.

## License

[MIT](LICENSE)
