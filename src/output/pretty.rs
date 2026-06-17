//! The classic colored terminal visualization, preserving the original ASCII
//! layout, color scheme and behaviour of the environment toggles.

use std::fs;
use std::path::PathBuf;

use crate::color::Palette;
use crate::http::HttpResult;
use crate::slo::Violation;
use crate::timing::TotalStats;

const HTTPS_TEMPLATE: &str = "\
  DNS Lookup   TCP Connection   TLS Handshake   Server Processing   Content Transfer
[   {a0000}  |     {a0001}    |    {a0002}    |      {a0003}      |      {a0004}     ]
             |                |               |                   |                  |
    namelookup:{b0000}        |               |                   |                  |
                        connect:{b0001}       |                   |                  |
                                    pretransfer:{b0002}           |                  |
                                                      starttransfer:{b0003}          |
                                                                                 total:{b0004}";

const HTTP_TEMPLATE: &str = "\
  DNS Lookup   TCP Connection   Server Processing   Content Transfer
[   {a0000}  |     {a0001}    |      {a0003}      |      {a0004}     ]
             |                |                   |                  |
    namelookup:{b0000}        |                   |                  |
                        connect:{b0001}           |                  |
                                      starttransfer:{b0003}          |
                                                                 total:{b0004}";

/// Behavioural toggles sourced from the `HTTPSTAT_*` environment variables.
pub struct PrettyOpts {
    pub show_ip: bool,
    pub show_body: bool,
    pub show_speed: bool,
    pub save_body: bool,
}

/// Render the full pretty report to stdout.
#[allow(clippy::too_many_arguments)]
pub fn render(
    p: &Palette,
    result: &HttpResult,
    https: bool,
    opts: &PrettyOpts,
    download_kbs: f64,
    upload_kbs: f64,
    violations: &[Violation],
    request_line: &str,
    stats: Option<&TotalStats>,
) {
    let r = &result.response;

    if !request_line.is_empty() {
        println!("{}", p.bold(request_line));
    }
    if opts.show_ip {
        println!(
            "Connected to {}:{} from {}:{}",
            p.cyan(&r.remote_ip),
            p.cyan(&r.remote_port),
            r.local_ip,
            r.local_port
        );
    }
    println!();

    // Status line: "HTTP/1.1 200 OK" -> green("HTTP") gray("/") cyan("1.1 200 OK").
    if let Some((proto, rest)) = r.status_line.split_once('/') {
        println!("{}{}{}", p.green(proto), p.gray(14, "/"), p.cyan(rest));
    } else {
        println!("{}", p.green(&r.status_line));
    }
    // Align header values into a column by padding each key to the widest one.
    let key_width = r.headers.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (k, v) in &r.headers {
        let key = format!("{:<width$}", format!("{k}:"), width = key_width + 1);
        println!("{}{}", p.gray(14, &key), p.cyan(&format!(" {v}")));
    }
    println!();

    // Body display / storage.
    if opts.show_body {
        const LIMIT: usize = 1024;
        let body = String::from_utf8_lossy(&result.body);
        let body = body.trim();
        if body.len() > LIMIT {
            println!("{}{}", &body[..LIMIT], p.cyan("..."));
            println!();
            let mut msg = format!(
                "{} is truncated ({LIMIT} out of {})",
                p.green("Body"),
                body.len()
            );
            if opts.save_body {
                if let Some(path) = save_body_to_tmp(&result.body) {
                    msg.push_str(&format!(", stored in: {}", path.display()));
                }
            }
            println!("{msg}");
        } else {
            println!("{body}");
        }
    } else if opts.save_body {
        if let Some(path) = save_body_to_tmp(&result.body) {
            println!("{} stored in: {}", p.green("Body"), path.display());
        }
    }

    // Timing breakdown box.
    let ranges = result.timings.ranges();
    let t = &result.timings;
    let template = if https { HTTPS_TEMPLATE } else { HTTP_TEMPLATE };

    let mut lines: Vec<String> = template.split('\n').map(|l| l.to_string()).collect();
    if let Some(first) = lines.first_mut() {
        *first = p.gray(16, first);
    }
    let mut stat = lines.join("\n");

    let a = |n: i64| p.cyan(&center(&format!("{n}ms"), 7));
    let b = |n: i64| p.cyan(&ljust(&format!("{n}ms"), 7));
    for (token, value) in [
        ("{a0000}", a(ranges.dns)),
        ("{a0001}", a(ranges.connection)),
        ("{a0002}", a(ranges.ssl)),
        ("{a0003}", a(ranges.server)),
        ("{a0004}", a(ranges.transfer)),
        ("{b0000}", b(t.namelookup_ms)),
        ("{b0001}", b(t.connect_ms)),
        ("{b0002}", b(t.pretransfer_ms)),
        ("{b0003}", b(t.starttransfer_ms)),
        ("{b0004}", b(t.total_ms)),
    ] {
        stat = stat.replace(token, &value);
    }
    println!();
    println!("{stat}");

    // A compact proportional phase bar, in the box's cyan accent.
    println!();
    println!("{}", phase_bar(p, &ranges, https));

    if let Some(s) = stats {
        println!();
        println!(
            "{}",
            p.gray(
                16,
                &format!(
                    "averaged over {} runs — total min {}ms · mean {}ms · max {}ms",
                    s.runs, s.min_ms, s.mean_ms, s.max_ms
                )
            )
        );
    }

    if opts.show_speed {
        println!("speed_download: {download_kbs:.1} KiB/s, speed_upload: {upload_kbs:.1} KiB/s");
    }

    if !violations.is_empty() {
        println!();
        for v in violations {
            println!(
                "{}",
                p.red(&format!(
                    "SLO VIOLATION: {} = {}ms (threshold: {}ms)",
                    v.key, v.actual_ms, v.threshold_ms
                ))
            );
        }
    }
}

/// A compact proportional bar of the five timing phases: each phase is labelled
/// and drawn with cyan blocks proportional to its share of the total, matching
/// the box's accent color. Zero-width phases (e.g. TLS on plain HTTP) are skipped.
fn phase_bar(p: &Palette, r: &crate::timing::Ranges, https: bool) -> String {
    const BLOCKS: i64 = 30;
    // On plain HTTP there is no TLS phase; the tiny ssl delta is measurement
    // jitter, so drop it like the HTTP timing template does.
    let ssl = if https { r.ssl } else { 0 };
    let phases = [
        ("DNS", r.dns),
        ("TCP", r.connection),
        ("TLS", ssl),
        ("Server", r.server),
        ("Transfer", r.transfer),
    ];
    let sum = phases.iter().map(|(_, v)| *v).sum::<i64>().max(1);

    let mut out = String::new();
    for (label, value) in phases {
        if value <= 0 {
            continue;
        }
        let n = ((value * BLOCKS + sum / 2) / sum).max(1) as usize;
        out.push_str(&format!(
            "{} {}  ",
            p.gray(12, label),
            p.cyan(&"█".repeat(n))
        ));
    }
    out.trim_end().to_string()
}

fn save_body_to_tmp(body: &[u8]) -> Option<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!("httpstat_body_{}.tmp", std::process::id()));
    fs::write(&path, body).ok().map(|_| path)
}

/// Center `s` in `width`, matching Python's `str.center` (extra space biased
/// left when both margin and width are odd) so the layout is identical.
fn center(s: &str, width: usize) -> String {
    if s.len() >= width {
        return s.to_string();
    }
    let marg = width - s.len();
    let left = marg / 2 + (marg & width & 1);
    let right = marg - left;
    format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
}

fn ljust(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_matches_python_semantics() {
        assert_eq!(center("5ms", 7), "  5ms  ");
        assert_eq!(center("100ms", 7), " 100ms ");
    }

    #[test]
    fn ljust_pads_right() {
        assert_eq!(ljust("5ms", 7), "5ms    ");
    }
}
