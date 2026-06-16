//! End-to-end CLI tests that don't require network access.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_httpstat"))
}

#[test]
fn version_flag_prints_version() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn no_args_prints_help_and_succeeds() {
    let out = bin().output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Usage"), "got: {stdout}");
}

#[test]
fn invalid_format_exits_with_usage_error() {
    let out = bin()
        .args(["--format", "xml", "http://example.com"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid format"), "got: {stderr}");
}

#[test]
fn invalid_slo_key_exits_with_usage_error() {
    let out = bin()
        .args(["--slo", "bogus=10", "http://example.com"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown SLO key"), "got: {stderr}");
}
