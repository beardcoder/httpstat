//! httpstat — curl statistics made simple, as a native Rust binary.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

use httpstat::app;
use httpstat::cli::{Cli, EnvOptions};
use httpstat::color::Palette;
use httpstat::error::{Error, Result, EXIT_OK};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(&cli) {
        Ok(code) => ExitCode::from(code),
        // A reader that closed the pipe (`httpstat … | head`) is not an error.
        Err(error) if error.is_broken_pipe() => ExitCode::from(EXIT_OK),
        Err(error) => {
            let palette = Palette::for_stderr();
            eprintln!("{}", palette.yellow(&format!("Error: {error}")));
            ExitCode::from(error.exit_code())
        }
    }
}

fn execute(cli: &Cli) -> Result<u8> {
    let env = EnvOptions::from_env()?;
    let palette = Palette::for_stdout();
    // One buffered, locked handle: the report is many small writes, and an
    // unbuffered stdout turns each of them into a syscall.
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let code = app::run(cli, &env, &palette, &mut out)?;
    out.flush()
        .map_err(|e| Error::io("could not flush the report to stdout", e))?;
    Ok(code)
}
