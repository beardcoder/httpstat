//! The classic colored terminal visualization, preserving the original ASCII
//! layout, color scheme and environment toggles.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::color::Palette;
use crate::output::Report;

const HTTPS_TEMPLATE: &str =
    "  DNS Lookup   TCP Connection   TLS Handshake   Server Processing   Content Transfer
│   {a0000}  │     {a0001}    │    {a0002}    │      {a0003}      │      {a0004}     │
             │                │               │                   │                  │
    namelookup:{b0000}        │               │                   │                  │
                        connect:{b0001}       │                   │                  │
                                    pretransfer:{b0002}           │                  │
                                                      starttransfer:{b0003}          │
                                                                                 total:{b0004}";

const HTTP_TEMPLATE: &str = "  DNS Lookup   TCP Connection   Server Processing   Content Transfer
│   {a0000}  │     {a0001}    │      {a0003}      │      {a0004}     │
             │                │                   │                  │
    namelookup:{b0000}        │                   │                  │
                        connect:{b0001}           │                  │
                                      starttransfer:{b0003}          │
                                                                 total:{b0004}";

/// How much of the body is shown inline by `HTTPSTAT_SHOW_BODY`.
const BODY_PREVIEW_LIMIT: usize = 1024;

/// Behavioural toggles sourced from the `HTTPSTAT_*` environment variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrettyOpts {
    pub show_ip: bool,
    pub show_body: bool,
    pub show_speed: bool,
    pub save_body: bool,
}

impl Default for PrettyOpts {
    fn default() -> Self {
        PrettyOpts {
            show_ip: true,
            show_body: false,
            show_speed: false,
            save_body: true,
        }
    }
}

/// Render the full report.
pub fn render(
    out: &mut impl Write,
    p: &Palette,
    report: &Report<'_>,
    opts: &PrettyOpts,
) -> io::Result<()> {
    let result = report.result;
    let head = &result.head;

    if !report.request_line.is_empty() {
        writeln!(out, "{}", p.bold(&report.request_line))?;
    }
    for hop in &result.hops {
        writeln!(
            out,
            "{}",
            p.gray(
                14,
                &format!("↪ {} redirected from {}", hop.status_code, hop.url)
            )
        )?;
    }
    if opts.show_ip {
        writeln!(
            out,
            "Connected to {}:{} from {}:{}",
            p.cyan(&result.remote.ip().to_string()),
            p.cyan(&result.remote.port().to_string()),
            result.local.ip(),
            result.local.port(),
        )?;
    }
    writeln!(out)?;

    // Status line: "HTTP/1.1 200 OK" -> green("HTTP") gray("/") cyan("1.1 200 OK").
    match head.status_line.split_once('/') {
        Some((proto, rest)) => {
            writeln!(out, "{}{}{}", p.green(proto), p.gray(14, "/"), p.cyan(rest))?
        }
        None => writeln!(out, "{}", p.green(&head.status_line))?,
    }
    // Align header values into a column by padding each name to the widest one.
    let name_width = head.headers.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (name, value) in head.headers.iter() {
        let name = format!("{:<width$}", format!("{name}:"), width = name_width + 1);
        writeln!(out, "{}{}", p.gray(14, &name), p.cyan(&format!(" {value}")))?;
    }
    writeln!(out)?;

    render_body(out, p, report, opts)?;

    writeln!(out)?;
    writeln!(out, "{}", timing_box(p, report))?;

    if let Some(stats) = &report.stats {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            p.gray(
                16,
                &format!(
                    "averaged over {} runs — total min {}ms · p50 {}ms · mean {}ms · p95 {}ms · max {}ms",
                    stats.runs, stats.min_ms, stats.p50_ms, stats.mean_ms, stats.p95_ms, stats.max_ms
                )
            )
        )?;
    }

    if opts.show_speed {
        writeln!(
            out,
            "speed_download: {:.1} KiB/s, speed_upload: {:.1} KiB/s",
            report.download_kbs, report.upload_kbs
        )?;
    }

    if !report.violations.is_empty() {
        writeln!(out)?;
        for violation in &report.violations {
            writeln!(out, "{}", p.red(&violation.to_string()))?;
        }
    }
    Ok(())
}

/// Show and/or store the response body, according to the toggles.
fn render_body(
    out: &mut impl Write,
    p: &Palette,
    report: &Report<'_>,
    opts: &PrettyOpts,
) -> io::Result<()> {
    let body = &report.result.body;
    if !opts.show_body {
        if opts.save_body && !body.bytes.is_empty() {
            match store_body(&body.bytes) {
                Ok(path) => writeln!(out, "{} stored in: {}", p.green("Body"), path.display())?,
                Err(e) => writeln!(
                    out,
                    "{}",
                    p.yellow(&format!("Body could not be stored: {e}"))
                )?,
            }
        }
        return Ok(());
    }

    let text = String::from_utf8_lossy(&body.bytes);
    let text = text.trim();
    if text.len() <= BODY_PREVIEW_LIMIT && !body.truncated() {
        writeln!(out, "{text}")?;
        return Ok(());
    }

    // Never slice mid-character: `text` is UTF-8 and the limit is a byte count.
    let preview = &text[..floor_char_boundary(text, BODY_PREVIEW_LIMIT)];
    writeln!(out, "{preview}{}", p.cyan("..."))?;
    writeln!(out)?;
    let mut note = format!(
        "{} is truncated ({BODY_PREVIEW_LIMIT} shown out of {} bytes)",
        p.green("Body"),
        body.total
    );
    if opts.save_body {
        match store_body(&body.bytes) {
            Ok(path) => {
                note.push_str(&format!(", stored in: {}", path.display()));
                if body.truncated() {
                    note.push_str(&format!(" (first {} bytes only)", body.bytes.len()));
                }
            }
            Err(e) => note.push_str(&format!(", but could not be stored: {e}")),
        }
    }
    writeln!(out, "{note}")
}

/// Fill the ASCII timing template with the measured phases.
fn timing_box(p: &Palette, report: &Report<'_>) -> String {
    let timings = &report.result.timings;
    let ranges = timings.ranges();
    let template = if report.result.https {
        HTTPS_TEMPLATE
    } else {
        HTTP_TEMPLATE
    };

    let mut lines: Vec<String> = template.split('\n').map(str::to_string).collect();
    if let Some(first) = lines.first_mut() {
        *first = p.gray(16, first);
    }
    let mut stat = lines.join("\n");

    let phase = |n: i64| p.cyan(&center(&format!("{n}ms"), 7));
    let milestone = |n: i64| p.cyan(&ljust(&format!("{n}ms"), 7));
    for (token, value) in [
        ("{a0000}", phase(ranges.dns)),
        ("{a0001}", phase(ranges.connection)),
        ("{a0002}", phase(ranges.ssl)),
        ("{a0003}", phase(ranges.server)),
        ("{a0004}", phase(ranges.transfer)),
        ("{b0000}", milestone(timings.namelookup_ms)),
        ("{b0001}", milestone(timings.connect_ms)),
        ("{b0002}", milestone(timings.pretransfer_ms)),
        ("{b0003}", milestone(timings.starttransfer_ms)),
        ("{b0004}", milestone(timings.total_ms)),
    ] {
        stat = stat.replace(token, &value);
    }
    // Dim the vertical strokes so the cyan timing values stand out.
    stat.replace('│', &p.gray(12, "│"))
}

/// Write the body to a fresh file in the temp directory.
///
/// The name includes a per-process counter as well as the pid so repeated runs
/// never collide, and `create_new` refuses to follow a file someone else placed
/// there first — a predictable temp path is otherwise a way to get a process to
/// overwrite a file on its author's behalf.
fn store_body(body: &[u8]) -> io::Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("httpstat_body_{pid}_{n}.tmp"));
        match create_private(&path) {
            Ok(mut file) => {
                file.write_all(body)?;
                file.flush()?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not find an unused temporary file name",
    ))
}

fn create_private(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A response body may hold session cookies or personal data; it has no
        // business being world-readable in a shared temp directory.
        options.mode(0o600);
    }
    options.open(path)
}

/// The largest index at or below `limit` that is a character boundary.
fn floor_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut index = limit;
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Center `s` in `width`, matching Python's `str.center` (the extra space is
/// biased left when both the margin and the width are odd) so the layout is
/// identical to the original tool's.
fn center(s: &str, width: usize) -> String {
    if s.len() >= width {
        return s.to_string();
    }
    let margin = width - s.len();
    let left = margin / 2 + (margin & width & 1);
    let right = margin - left;
    format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
}

fn ljust(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::body::Body;
    use crate::testing;

    fn opts() -> PrettyOpts {
        PrettyOpts {
            save_body: false,
            ..PrettyOpts::default()
        }
    }

    fn render_to_string(report: &Report<'_>, opts: &PrettyOpts) -> String {
        let mut buffer = Vec::new();
        render(&mut buffer, &Palette::plain(), report, opts).expect("writing to a Vec cannot fail");
        String::from_utf8(buffer).expect("output is UTF-8")
    }

    #[test]
    fn renders_the_full_https_report() {
        let result = testing::result();
        let text = render_to_string(&testing::report(&result), &opts());
        assert_eq!(
            text,
            "\
GET https://example.com/
Connected to 93.184.216.34:443 from 192.168.1.5:54321

HTTP/1.1 200 OK
Content-Type: text/plain
Server:       nginx


  DNS Lookup   TCP Connection   TLS Handshake   Server Processing   Content Transfer
│     5ms    │       10ms     │      15ms     │        50ms       │        20ms      │
             │                │               │                   │                  │
    namelookup:5ms            │               │                   │                  │
                        connect:15ms          │                   │                  │
                                    pretransfer:30ms              │                  │
                                                      starttransfer:80ms             │
                                                                                 total:100ms  
"
        );
    }

    #[test]
    fn plain_http_uses_the_template_without_a_tls_column() {
        let mut result = testing::result();
        result.https = false;
        result.final_url = "http://example.com/".into();
        let text = render_to_string(&testing::report(&result), &opts());
        assert!(!text.contains("TLS Handshake"), "{text}");
        assert!(!text.contains("pretransfer:"), "{text}");
        assert!(
            text.contains("DNS Lookup   TCP Connection   Server Processing"),
            "{text}"
        );
    }

    #[test]
    fn the_ip_line_can_be_switched_off() {
        let result = testing::result();
        let hidden = PrettyOpts {
            show_ip: false,
            ..opts()
        };
        assert!(!render_to_string(&testing::report(&result), &hidden).contains("Connected to"));
        assert!(render_to_string(&testing::report(&result), &opts()).contains("Connected to"));
    }

    #[test]
    fn speed_is_shown_only_when_asked_for() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.download_kbs = 1234.5;
        report.upload_kbs = 0.0;
        let shown = PrettyOpts {
            show_speed: true,
            ..opts()
        };
        assert!(render_to_string(&report, &shown)
            .contains("speed_download: 1234.5 KiB/s, speed_upload: 0.0 KiB/s"));
        assert!(!render_to_string(&report, &opts()).contains("speed_download"));
    }

    #[test]
    fn the_body_is_printed_when_show_body_is_set() {
        let result = testing::result();
        let with_body = PrettyOpts {
            show_body: true,
            ..opts()
        };
        let text = render_to_string(&testing::report(&result), &with_body);
        assert!(text.contains("\nhello world\n"), "{text}");
    }

    #[test]
    fn a_long_body_is_truncated_without_splitting_a_character() {
        // A multi-byte character straddling the 1024-byte preview limit used to
        // panic on a byte-index slice.
        let mut result = testing::result();
        let text: String = "a".repeat(BODY_PREVIEW_LIMIT - 1) + "ü" + &"b".repeat(64);
        let bytes = text.into_bytes();
        result.body = Body {
            total: bytes.len(),
            bytes,
        };
        let with_body = PrettyOpts {
            show_body: true,
            ..opts()
        };
        let rendered = render_to_string(&testing::report(&result), &with_body);
        assert!(rendered.contains(&format!("{}...", "a".repeat(BODY_PREVIEW_LIMIT - 1))));
        assert!(
            rendered.contains("Body is truncated (1024 shown out of 1089 bytes)"),
            "{rendered}"
        );
    }

    #[test]
    fn a_body_larger_than_the_retention_limit_reports_its_real_size() {
        let mut result = testing::result();
        result.body = Body {
            bytes: vec![b'x'; 2048],
            total: 10_000_000,
        };
        result.download_bytes = 10_000_000;
        let with_body = PrettyOpts {
            show_body: true,
            ..opts()
        };
        let text = render_to_string(&testing::report(&result), &with_body);
        assert!(text.contains("out of 10000000 bytes"), "{text}");
    }

    #[test]
    fn followed_redirects_are_listed_above_the_status_line() {
        let mut result = testing::result();
        result.hops = vec![testing::hop("http://example.com/", 301)];
        let text = render_to_string(&testing::report(&result), &opts());
        assert!(
            text.contains("↪ 301 redirected from http://example.com/"),
            "{text}"
        );
    }

    #[test]
    fn slo_violations_are_listed_at_the_end() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.violations = vec![testing::violation("total", 50, 100)];
        let text = render_to_string(&report, &opts());
        assert!(
            text.trim_end()
                .ends_with("SLO VIOLATION: total = 100ms (threshold: 50ms)"),
            "{text}"
        );
    }

    #[test]
    fn multiple_runs_add_the_distribution_line() {
        let result = testing::result();
        let mut report = testing::report(&result);
        report.runs = 3;
        report.stats = Some(testing::stats(&[100, 200, 300]));
        let text = render_to_string(&report, &opts());
        assert!(
            text.contains("averaged over 3 runs — total min 100ms · p50 200ms · mean 200ms · p95 300ms · max 300ms"),
            "{text}"
        );
    }

    #[test]
    fn colors_are_emitted_only_with_an_enabled_palette() {
        let result = testing::result();
        let report = testing::report(&result);
        let mut colored = Vec::new();
        render(&mut colored, &Palette::new(true), &report, &opts()).unwrap();
        assert!(String::from_utf8_lossy(&colored).contains("\x1b["));
        assert!(!render_to_string(&report, &opts()).contains('\x1b'));
    }

    #[test]
    fn a_stored_body_is_written_privately_and_never_clobbers_an_existing_file() {
        let first = store_body(b"secret").unwrap();
        let second = store_body(b"secret").unwrap();
        assert_ne!(first, second, "each call must claim its own file");
        assert_eq!(std::fs::read(&first).unwrap(), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "body files must not be world-readable");
        }
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }

    #[test]
    fn center_matches_python_semantics() {
        assert_eq!(center("5ms", 7), "  5ms  ");
        assert_eq!(center("100ms", 7), " 100ms ");
        assert_eq!(center("1000000ms", 7), "1000000ms");
    }

    #[test]
    fn ljust_pads_right() {
        assert_eq!(ljust("5ms", 7), "5ms    ");
        assert_eq!(ljust("1234567ms", 7), "1234567ms");
    }

    #[test]
    fn floor_char_boundary_never_splits_a_character() {
        let s = "aüb";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 1);
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 99), s.len());
    }
}
