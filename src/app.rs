//! Application orchestration: resolve options, issue the requests, render.
//!
//! Everything here is parameterized over the output sink and the environment so
//! the whole pipeline can be exercised without a terminal.

use std::fs;
use std::io::{IsTerminal, Write};

use clap::CommandFactory;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::cli::{Cli, EnvOptions};
use crate::color::Palette;
use crate::error::{Error, Result, EXIT_OK, EXIT_SLO};
use crate::http::{self, HttpResult};
use crate::output::{json, kbs, pretty, Format, Report};
use crate::slo::Slo;
use crate::timing::{Timings, TotalStats};

/// Run the tool and return the process exit code.
pub fn run(cli: &Cli, env: &EnvOptions, palette: &Palette, out: &mut impl Write) -> Result<u8> {
    let Some(url) = cli.url.as_deref() else {
        // No URL: behave like the original and print the help text.
        let help = Cli::command().render_help().to_string();
        return write(out, format_args!("{help}\n")).map(|()| EXIT_OK);
    };

    let format = resolve_format(cli, env)?;
    let slo = cli.slo.as_deref().map(Slo::parse).transpose()?;
    let opts = cli.request_options()?;
    let runs = cli.count;

    if env.debug {
        eprintln!(
            "[debug] url={url} method={} format={format} follow={} max_redirects={} insecure={} runs={runs}",
            opts.method, opts.follow_redirects, opts.max_redirects, opts.insecure
        );
    }

    let results = perform(url, &opts, runs)?;
    let (result, stats) = aggregate(results);

    let violations = slo
        .as_ref()
        .map(|s| s.check(&result.timings))
        .unwrap_or_default();
    let exit_code = if violations.is_empty() {
        EXIT_OK
    } else {
        EXIT_SLO
    };

    let report = Report {
        request_line: format!("{} {}", opts.method, url),
        download_kbs: kbs(result.download_bytes, result.transfer_secs),
        // The request body goes out before the response starts arriving, so it
        // is rated over the whole request the way curl's speed_upload is.
        upload_kbs: kbs(result.upload_bytes, result.timings.total_ms as f64 / 1000.0),
        violations,
        slo_requested: slo.is_some(),
        runs,
        stats,
        exit_code,
        result: &result,
    };

    match format {
        Format::Json | Format::Jsonl => {
            let text = json::render(&report, format.is_indented_json());
            write(out, format_args!("{text}\n"))?;
        }
        Format::Pretty => {
            let opts = pretty::PrettyOpts {
                show_ip: env.show_ip,
                show_body: env.show_body,
                show_speed: env.show_speed,
                save_body: env.save_body,
            };
            pretty::render(out, palette, &report, &opts).map_err(broken_pipe_aware)?;
        }
    }

    if let Some(path) = &cli.save {
        // The saved file is always JSON, whatever the terminal format is.
        let text = json::render(&report, true);
        fs::write(path, format!("{text}\n"))
            .map_err(|e| Error::io(format!("could not write {path}"), e))?;
    }

    Ok(exit_code)
}

/// `HTTPSTAT_METRICS_ONLY` is the pre-`--format` way of asking for JSON; an
/// explicit `--format` always wins over it.
fn resolve_format(cli: &Cli, env: &EnvOptions) -> Result<Format> {
    let format = cli.format()?;
    if env.metrics_only && format == Format::Pretty {
        return Ok(Format::Json);
    }
    Ok(format)
}

/// Issue the request `runs` times, showing progress when it is worth showing.
fn perform(url: &str, opts: &http::RequestOptions, runs: u32) -> Result<Vec<HttpResult>> {
    let progress = progress_bar(runs);
    let mut results = Vec::with_capacity(runs as usize);
    for run in 1..=runs {
        match http::fetch(url, opts) {
            Ok(result) => {
                results.push(result);
                if let Some(bar) = &progress {
                    bar.inc(1);
                }
            }
            Err(error) => {
                if let Some(bar) = &progress {
                    bar.finish_and_clear();
                }
                return Err(match (runs, error) {
                    (1, error) => error,
                    // Which run failed matters when only some of them do.
                    (_, Error::Request(message)) => {
                        Error::request(format!("run {run} of {runs} failed: {message}"))
                    }
                    (_, error) => error,
                });
            }
        }
    }
    if let Some(bar) = &progress {
        bar.finish_and_clear();
    }
    Ok(results)
}

/// Collapse repeated runs into one result carrying the averaged timings.
///
/// The response shown is the last one; only the numbers are aggregated.
fn aggregate(mut results: Vec<HttpResult>) -> (HttpResult, Option<TotalStats>) {
    let samples: Vec<Timings> = results.iter().map(|r| r.timings).collect();
    let runs = results.len();
    let mut result = results.pop().expect("at least one run is always performed");
    if runs <= 1 {
        return (result, None);
    }

    let runs_f = runs as f64;
    result.timings = Timings::mean(&samples);
    result.transfer_secs = results
        .iter()
        .map(|r| r.transfer_secs)
        .chain(std::iter::once(result.transfer_secs))
        .sum::<f64>()
        / runs_f;
    let total_bytes: usize = results
        .iter()
        .map(|r| r.download_bytes)
        .chain(std::iter::once(result.download_bytes))
        .sum();
    result.download_bytes = (total_bytes as f64 / runs_f).round() as usize;
    (result, TotalStats::from_samples(&samples))
}

/// A progress bar for multi-run requests, shown only when there is more than one
/// run and stderr is a terminal, so piped and JSON output stay clean.
fn progress_bar(runs: u32) -> Option<ProgressBar> {
    if runs <= 1 || !std::io::stderr().is_terminal() {
        return None;
    }
    let style = ProgressStyle::with_template(
        "  {spinner:.cyan} {pos}/{len} requests {wide_bar:.cyan/blue} {elapsed}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("█▉░");
    let bar = ProgressBar::with_draw_target(Some(runs as u64), ProgressDrawTarget::stderr())
        .with_style(style);
    Some(bar)
}

/// Write to the output sink, tolerating a closed pipe.
fn write(out: &mut impl Write, args: std::fmt::Arguments<'_>) -> Result<()> {
    out.write_fmt(args).map_err(broken_pipe_aware)
}

/// `httpstat … | head` closes the pipe early; that is the reader's business,
/// not a failure of ours. Tagging it here lets the binary exit quietly with 0
/// instead of printing an I/O error nobody asked about.
fn broken_pipe_aware(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        return Error::io("output pipe closed", error);
    }
    Error::io("could not write the report", error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("httpstat").chain(args.iter().copied())).unwrap()
    }

    fn run_to_string(cli: &Cli, env: &EnvOptions) -> (u8, String) {
        let mut buffer = Vec::new();
        let code = run(cli, env, &Palette::plain(), &mut buffer).expect("run should succeed");
        (code, String::from_utf8(buffer).unwrap())
    }

    #[test]
    fn without_a_url_it_prints_help_and_succeeds() {
        let (code, text) = run_to_string(&cli(&[]), &EnvOptions::default());
        assert_eq!(code, EXIT_OK);
        assert!(text.contains("Usage:"), "{text}");
        assert!(text.contains("--slo"), "{text}");
    }

    #[test]
    fn invalid_options_are_reported_before_any_request_is_attempted() {
        let env = EnvOptions::default();
        let palette = Palette::plain();
        for args in [
            vec!["example.com", "-f", "xml"],
            vec!["example.com", "--slo", "bogus=1"],
            vec!["example.com", "-H", "no-colon"],
            vec!["example.com", "--max-time", "0"],
        ] {
            let err = run(&cli(&args), &env, &palette, &mut Vec::new()).unwrap_err();
            assert_eq!(err.exit_code(), crate::error::EXIT_USAGE, "{args:?}");
        }
    }

    #[test]
    fn metrics_only_selects_json_but_an_explicit_format_wins() {
        let env = EnvOptions {
            metrics_only: true,
            ..EnvOptions::default()
        };
        assert_eq!(
            resolve_format(&cli(&["example.com"]), &env).unwrap(),
            Format::Json
        );
        assert_eq!(
            resolve_format(&cli(&["example.com", "-f", "jsonl"]), &env).unwrap(),
            Format::Jsonl
        );
        let plain = EnvOptions::default();
        assert_eq!(
            resolve_format(&cli(&["example.com"]), &plain).unwrap(),
            Format::Pretty
        );
    }

    #[test]
    fn a_single_run_is_passed_through_untouched() {
        let (result, stats) = aggregate(vec![testing::result()]);
        assert_eq!(result.timings, testing::timings());
        assert_eq!(stats, None);
    }

    #[test]
    fn repeated_runs_are_averaged_and_summarized() {
        let make = |total_ms: i64, bytes: usize, secs: f64| {
            let mut r = testing::result();
            r.timings = Timings {
                total_ms,
                ..testing::timings()
            };
            r.download_bytes = bytes;
            r.transfer_secs = secs;
            r
        };
        let (result, stats) = aggregate(vec![
            make(100, 1000, 0.1),
            make(200, 2000, 0.2),
            make(300, 3000, 0.3),
        ]);
        assert_eq!(result.timings.total_ms, 200);
        assert_eq!(result.download_bytes, 2000);
        assert!((result.transfer_secs - 0.2).abs() < 1e-9);
        let stats = stats.unwrap();
        assert_eq!(
            (stats.runs, stats.min_ms, stats.mean_ms, stats.max_ms),
            (3, 100, 200, 300)
        );
    }

    #[test]
    fn no_progress_bar_is_drawn_for_a_single_run() {
        assert!(progress_bar(1).is_none());
    }

    #[test]
    fn a_closed_pipe_is_not_treated_as_a_report_failure() {
        let broken = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed");
        assert!(broken_pipe_aware(broken)
            .to_string()
            .contains("pipe closed"));
    }
}
