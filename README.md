# httpstat

![screenshot](screenshot.png)

httpstat visualizes HTTP request timings in a way of beauty and clarity.

It is a **single static binary 🌟** written in **native Rust 🦀** with **no runtime
dependency 👏** — the HTTP request is performed in-process (no `curl` binary), and
TLS is handled by a pure-Rust stack (rustls + ring), so the Linux builds link
statically with no OpenSSL.

## Features

- **Beautiful terminal output** — timing breakdown of DNS, TCP, TLS, server processing, and content transfer
- **Structured JSON output** — `--format json` / `jsonl` for machine consumption with a stable v1 schema
- **SLO threshold checking** — `--slo total=500,connect=100` exits with code 4 on violation
- **Save results to file** — `--save path.json` for multi-step workflows
- **NO_COLOR support** — respects the [NO_COLOR](https://no-color.org) convention

## Installation

### Download a prebuilt binary

Grab the archive for your platform from the
[latest release](https://github.com/beardcoder/httpstat/releases/latest),
extract it, and put the `httpstat` binary on your `PATH`:

```bash
# example: Linux x86_64
curl -fsSL -o httpstat.tar.gz \
  https://github.com/beardcoder/httpstat/releases/latest/download/httpstat-x86_64-unknown-linux-musl.tar.gz
tar -xzf httpstat.tar.gz
sudo install httpstat /usr/local/bin/
```

Available targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
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

## Usage

```bash
httpstat httpbin.org/get
```

A bare host is accepted and defaults to `http://`. The request is issued
in-process — only the documented options below are supported (not arbitrary
curl flags). Run `httpstat --help` for the full list.

| Option | Description |
| --- | --- |
| `-f, --format <FORMAT>` | Output format: `pretty` (default), `json`, `jsonl` |
| `--slo <SPEC>` | SLO thresholds, e.g. `total=500,connect=100` (exit 4 on violation) |
| `--save <PATH>` | Save the structured JSON result to a file |
| `-X, --request <METHOD>` | HTTP method (defaults to GET, or POST when `--data` is given) |
| `-H, --header <HEADER>` | Extra request header `Name: Value` (repeatable) |
| `-d, --data <DATA>` | Request body data |
| `-L, --location` | Follow HTTP redirects |
| `-k, --insecure` | Skip TLS certificate verification |
| `-A, --user-agent <UA>` | User-Agent header value |
| `--connect-timeout <SECONDS>` | Maximum time allowed for the TCP connection |
| `--max-time <SECONDS>` | Maximum total time allowed for the transfer |

```bash
httpstat httpbin.org/post -X POST -d '{"a":"b"}' -H 'Content-Type: application/json' -L
```

### Structured Output

Use `--format` (`-f`) to get machine-readable output:

```bash
httpstat httpbin.org/get --format json
```

```json
{
  "schema_version": 1,
  "url": "httpbin.org/get",
  "ok": true,
  "exit_code": 0,
  "response": {
    "status_line": "HTTP/1.1 200 OK",
    "status_code": 200,
    "remote_ip": "...",
    "remote_port": "443",
    "headers": {"Content-Type": "application/json", "Server": "nginx", "...": "..."}
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

Use `--format jsonl` for compact single-line JSON (useful for log pipelines).

### SLO Thresholds

Check response times against thresholds. Exits with code `4` on violation:

```bash
httpstat httpbin.org/get --slo total=500,connect=100,ttfb=200
```

Supported keys: `total`, `connect`, `ttfb` (time to first byte), `dns`, `tls`.

In pretty mode, violations are printed in red at the end of the output.
In JSON mode, violations appear in the `slo` field:

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

### Save Results

Write structured JSON output to a file (works with any `--format`):

```bash
httpstat httpbin.org/get --save result.json
httpstat httpbin.org/get --format json --save result.json
```

### Environment Variables

Run `httpstat --help` to see the full explanation. All booleans accept
`1/true/yes/on` and `0/false/no/off`.

| Variable | Default | Effect |
| --- | --- | --- |
| `HTTPSTAT_SHOW_BODY` | `false` | Show response body (truncated to 1024 bytes) |
| `HTTPSTAT_SHOW_IP` | `true` | Show remote/local IP and port |
| `HTTPSTAT_SHOW_SPEED` | `false` | Show download/upload speed |
| `HTTPSTAT_SAVE_BODY` | `true` | Store body in a temp file |
| `HTTPSTAT_METRICS_ONLY` | `false` | Equivalent to `--format json` (kept for compatibility) |
| `HTTPSTAT_DEBUG` | `false` | Print resolved options to stderr |
| `NO_COLOR` | unset | When set to any value, disables colored output ([no-color.org](https://no-color.org)) |

For convenience, export these in your `.zshrc` or `.bashrc`:

```bash
export HTTPSTAT_SHOW_IP=false
export HTTPSTAT_SHOW_SPEED=true
export HTTPSTAT_SAVE_BODY=false
```

## Development

```bash
make test     # cargo test
make fmt      # cargo fmt
make clippy   # cargo clippy -D warnings
make build    # release build for the host
make build-all  # cross-compile all release targets into dist/
```

CI runs fmt, clippy and tests on every push/PR; pushing a `v*` tag builds the
cross-platform binaries and publishes a GitHub release.

## Credits

A native Rust port of the original Python
[reorx/httpstat](https://github.com/reorx/httpstat) by Reorx.

## License

[MIT](LICENSE)
