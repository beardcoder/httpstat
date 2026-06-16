//! ANSI color helpers. Mirrors the original Python `make_color` behaviour and
//! respects both TTY detection and the NO_COLOR convention (https://no-color.org).

use std::io::IsTerminal;

/// A palette that knows whether coloring is enabled. When disabled (not a TTY,
/// or `NO_COLOR` set), every helper returns the input string unchanged.
#[derive(Clone, Copy)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    /// Build a palette from the environment: colors are on only when stdout is a
    /// terminal and `NO_COLOR` is unset.
    pub fn from_env() -> Self {
        let enabled = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Palette { enabled }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }

    /// 256-color grayscale, matching the original `grayscale[n]` (n in 0..=23).
    pub fn gray(&self, n: u8, s: &str) -> String {
        self.paint(&format!("38;5;{}", 232 + n.min(23)), s)
    }
}
