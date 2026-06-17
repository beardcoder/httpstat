//! httpstat — curl statistics made simple, as a native Rust binary.

mod cli;
mod color;
mod http;
mod output;
mod slo;
mod timing;

use std::fs;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};

use cli::Cli;
use color::Palette;
use http::{HttpResult, RequestOptions};
use output::{json, pretty, Format};
use slo::Slo;
use timing::{Timings, TotalStats};

const EXIT_USAGE: u8 = 1;
const EXIT_SLO: u8 = 4;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let palette = Palette::from_env();
    match run(cli, &palette) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("{}", palette.yellow(&format!("Error: {message}")));
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// A progress bar for multi-run requests, shown only when there is more than one
/// run and stderr is a terminal (so piped/JSON output stays clean).
fn make_progress(runs: usize) -> Option<ProgressBar> {
    if runs <= 1 || !std::io::stderr().is_terminal() {
        return None;
    }
    let style = ProgressStyle::with_template(
        "  {spinner:.cyan} {pos}/{len} requests {wide_bar:.cyan/blue} {elapsed}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar())
    .progress_chars("█▉░");
    let pb = ProgressBar::new(runs as u64).with_style(style);
    Some(pb)
}

fn run(cli: Cli, palette: &Palette) -> Result<u8, String> {
    let Some(url) = cli.url.clone() else {
        // No URL given: behave like the original and print help.
        use clap::CommandFactory;
        Cli::command().print_help().ok();
        println!();
        return Ok(0);
    };

    // Environment toggles.
    let show_body = cli::env_bool("HTTPSTAT_SHOW_BODY", false)?;
    let show_ip = cli::env_bool("HTTPSTAT_SHOW_IP", true)?;
    let show_speed = cli::env_bool("HTTPSTAT_SHOW_SPEED", false)?;
    let save_body = cli::env_bool("HTTPSTAT_SAVE_BODY", true)?;
    let metrics_only = cli::env_bool("HTTPSTAT_METRICS_ONLY", false)?;
    let debug = cli::env_bool("HTTPSTAT_DEBUG", false)?;

    // Output format, with the metrics-only backward-compat shortcut.
    let mut format = Format::parse(&cli.format)?;
    if metrics_only && format == Format::Pretty {
        format = Format::Json;
    }

    let slo = cli.slo.as_deref().map(Slo::parse).transpose()?;

    // Build request options.
    let body = cli.data.clone().map(String::into_bytes);
    let method = cli::effective_method(&cli.method, body.is_some());
    let headers = cli
        .headers
        .iter()
        .map(|h| cli::parse_header(h))
        .collect::<Result<Vec<_>, _>>()?;
    let opts = RequestOptions {
        method,
        headers,
        body,
        follow_redirects: cli.follow,
        insecure: cli.insecure,
        user_agent: cli.user_agent.clone(),
        connect_timeout: cli.connect_timeout.map(Duration::from_secs_f64),
        max_time: cli.max_time.map(Duration::from_secs_f64),
    };

    let runs = cli.count as usize;

    if debug {
        eprintln!(
            "[debug] url={url} method={} format={format:?} follow={} insecure={} runs={runs}",
            opts.method, opts.follow_redirects, opts.insecure
        );
    }

    // Run the request `runs` times; report averaged timings over all of them.
    // A progress bar (stderr, TTY only) gives feedback while the runs proceed.
    let progress = make_progress(runs);
    let mut results: Vec<HttpResult> = Vec::with_capacity(runs);
    for _ in 0..runs {
        match http::fetch(&url, &opts) {
            Ok(res) => {
                results.push(res);
                if let Some(pb) = &progress {
                    pb.inc(1);
                }
            }
            Err(e) => {
                if let Some(pb) = &progress {
                    pb.finish_and_clear();
                }
                return Err(e);
            }
        }
    }
    if let Some(pb) = &progress {
        pb.finish_and_clear();
    }
    let timing_samples: Vec<Timings> = results.iter().map(|r| r.timings).collect();
    let stats = (runs > 1).then(|| TotalStats::from_samples(&timing_samples));

    // The last run carries the response/body shown; aggregate fields are averaged.
    let mean_timings = Timings::mean(&timing_samples);
    let mean_transfer = results.iter().map(|r| r.transfer_secs).sum::<f64>() / runs as f64;
    let mean_bytes =
        (results.iter().map(|r| r.download_bytes).sum::<usize>() as f64 / runs as f64).round();
    let mut result = results.pop().expect("count >= 1 guarantees one run");
    if runs > 1 {
        result.timings = mean_timings;
        result.transfer_secs = mean_transfer;
        result.download_bytes = mean_bytes as usize;
    }

    let download_kbs = output::kbs(result.download_bytes, result.transfer_secs);
    let upload_kbs = 0.0;
    let request_line = format!("{} {url}", opts.method);

    let violations = slo
        .as_ref()
        .map(|s| s.check(&result.timings))
        .unwrap_or_default();
    let exit_code: u8 = if !violations.is_empty() { EXIT_SLO } else { 0 };
    let slo_slice = slo.as_ref().map(|_| violations.as_slice());

    match format {
        Format::Json | Format::Jsonl => {
            let text = json::render(
                &result,
                slo_slice,
                exit_code as i32,
                download_kbs,
                upload_kbs,
                format == Format::Json,
                runs as u32,
                stats.as_ref(),
            );
            println!("{text}");
            if let Some(path) = &cli.save {
                fs::write(path, format!("{text}\n"))
                    .map_err(|e| format!("could not write {path}: {e}"))?;
            }
        }
        Format::Pretty => {
            let https = result.final_url.starts_with("https://");
            let pretty_opts = pretty::PrettyOpts {
                show_ip,
                show_body,
                show_speed,
                save_body,
            };
            pretty::render(
                palette,
                &result,
                https,
                &pretty_opts,
                download_kbs,
                upload_kbs,
                &violations,
                &request_line,
                stats.as_ref(),
            );
            if let Some(path) = &cli.save {
                let text = json::render(
                    &result,
                    slo_slice,
                    exit_code as i32,
                    download_kbs,
                    upload_kbs,
                    true,
                    runs as u32,
                    stats.as_ref(),
                );
                fs::write(path, format!("{text}\n"))
                    .map_err(|e| format!("could not write {path}: {e}"))?;
            }
        }
    }

    Ok(exit_code)
}
