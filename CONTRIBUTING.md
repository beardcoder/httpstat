# Contributing

Thanks for taking the time. This is a small tool with a narrow job, so the bar is
mostly "does it keep the measurements honest and the output readable".

## Getting set up

```bash
git clone https://github.com/beardcoder/httpstat
cd httpstat
make check     # fmt --check, clippy -D warnings, and the tests
```

Rust 1.85 or newer is required (`rust-version` in `Cargo.toml`).

## Before you open a pull request

Run the same gate CI runs:

```bash
make check
```

That is `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test --all`. All three must be clean.

## Tests

- **No network.** The test suite must pass on a machine with no route to the
  internet. Integration tests use the loopback server in `tests/support/mod.rs`,
  which binds `127.0.0.1:0`, replays canned raw responses and records the
  requests it received.
- **Unit tests live next to the code** in `#[cfg(test)] mod tests`. Shared
  fixtures are in `src/testing.rs`.
- **A bug fix comes with the test that would have caught it.** Most of the
  interesting behaviour here — response framing, timeouts, redirect semantics —
  is only observable end to end, which is what the loopback server is for.

## Style

- `cargo fmt` decides formatting; do not hand-align.
- Comments explain *why*, not *what*. If a line looks strange, the comment should
  say what would go wrong without it.
- Fallible functions return `crate::error::Result`. A bad flag or environment
  value is `Error::Usage` (exit 2); anything that goes wrong on the wire is
  `Error::Request` (exit 1).
- Renderers write into an `impl Write`. Nothing below `main.rs` should call
  `println!`.

## Changing the JSON output

`src/output/json.rs` defines the schema and is the only place that should.
Adding a field is fine within `schema_version 1`; renaming or removing one is a
breaking change and bumps the version. Document it in `CHANGELOG.md` and README.

## Releasing

1. Bump `version` in `Cargo.toml` and run `cargo update -p httpstat` so the
   lockfile follows.
2. Add a `CHANGELOG.md` entry.
3. Push a `v*` tag. `.github/workflows/release.yml` builds the four platform
   binaries with checksums and publishes the GitHub release.
