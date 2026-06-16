//! httpstat — curl statistics made simple, as a native Rust binary.

mod cli;
mod color;
mod http;
mod output;
mod slo;
mod timing;

use std::fs;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use cli::Cli;
use color::Palette;
use http::RequestOptions;
use output::{json, pretty, Format};
use slo::Slo;

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

    if debug {
        eprintln!(
            "[debug] url={url} method={} format={format:?} follow={} insecure={}",
            opts.method, opts.follow_redirects, opts.insecure
        );
    }

    let result = http::fetch(&url, &opts)?;

    let download_kbs = output::kbs(result.download_bytes, result.transfer_secs);
    let upload_kbs = 0.0;

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
            );
            if let Some(path) = &cli.save {
                let text = json::render(
                    &result,
                    slo_slice,
                    exit_code as i32,
                    download_kbs,
                    upload_kbs,
                    true,
                );
                fs::write(path, format!("{text}\n"))
                    .map_err(|e| format!("could not write {path}: {e}"))?;
            }
        }
    }

    Ok(exit_code)
}
