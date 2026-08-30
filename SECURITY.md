# Security Policy

## Supported versions

Fixes go into the latest release. Please upgrade before reporting an issue with
an older one.

## Reporting a vulnerability

Please report security issues privately through
[GitHub's private vulnerability reporting](https://github.com/beardcoder/httpstat/security/advisories/new)
rather than in a public issue. Include what you did, what happened, and the
version (`httpstat --version`). You can expect an acknowledgement within a few
days.

## Scope and threat model

`httpstat` is a command-line client: it takes a URL from its own user and sends
one request. The interesting boundary is the response, which comes from a server
the user chose but does not necessarily trust. Things in scope:

- **Response parsing.** A malicious or broken server should not be able to make
  the tool crash, hang indefinitely, or consume unbounded memory. Response heads
  are capped, chunk headers are capped, and bodies are counted in full but only
  partly retained.
- **Request construction.** A header value or method must not be able to inject
  additional request lines. CR, LF and other control characters are rejected in
  `-H` and `--user-agent` values.
- **Credential handling.** `Authorization`, `Cookie` and `Proxy-Authorization`
  are dropped when a redirect crosses to a different origin.
- **Local artefacts.** Body files written to the temp directory are created with
  a non-guessable name, mode `0600` on Unix, and never overwrite an existing file.

Out of scope, by design:

- `-k/--insecure` disables certificate verification. That is what it is for.
- `--cacert` and `$SSL_CERT_FILE` replace the built-in root store. Pointing them
  at an untrusted bundle is equivalent to trusting that bundle.
- The tool does not sandbox the URL it is given; it will connect to loopback and
  private-range addresses if asked to.
