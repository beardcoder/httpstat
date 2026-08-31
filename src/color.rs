//! ANSI coloring, gated on TTY detection and the
//! [NO_COLOR](https://no-color.org) convention.

use std::io::IsTerminal;

/// A palette that knows whether coloring is enabled. When disabled, every
/// helper returns its input unchanged, so callers never branch on color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    pub const fn new(enabled: bool) -> Self {
        Palette { enabled }
    }

    /// A palette that never emits escape sequences. Useful for tests and for
    /// anything written to a file.
    pub const fn plain() -> Self {
        Palette::new(false)
    }

    /// Colors for stdout: on only when stdout is a terminal and `NO_COLOR` is
    /// unset or empty.
    pub fn for_stdout() -> Self {
        Palette::new(std::io::stdout().is_terminal() && !no_color())
    }

    /// Colors for stderr, decided independently of stdout: piping the report
    /// into a file should not strip the color from error messages.
    pub fn for_stderr() -> Self {
        Palette::new(std::io::stderr().is_terminal() && !no_color())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
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

/// The NO_COLOR convention: any non-empty value disables color.
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_palette_passes_text_through_untouched() {
        let p = Palette::plain();
        assert!(!p.is_enabled());
        assert_eq!(p.red("boom"), "boom");
        assert_eq!(p.gray(14, "/"), "/");
        assert_eq!(p.bold("x"), "x");
    }

    #[test]
    fn an_enabled_palette_wraps_text_in_escape_codes() {
        let p = Palette::new(true);
        assert_eq!(p.red("boom"), "\x1b[31mboom\x1b[0m");
        assert_eq!(p.green("ok"), "\x1b[32mok\x1b[0m");
        assert_eq!(p.cyan("v"), "\x1b[36mv\x1b[0m");
        assert_eq!(p.yellow("!"), "\x1b[33m!\x1b[0m");
        assert_eq!(p.bold("b"), "\x1b[1mb\x1b[0m");
    }

    #[test]
    fn grayscale_is_offset_from_232_and_clamped() {
        let p = Palette::new(true);
        assert_eq!(p.gray(0, "x"), "\x1b[38;5;232mx\x1b[0m");
        assert_eq!(p.gray(14, "x"), "\x1b[38;5;246mx\x1b[0m");
        assert_eq!(p.gray(200, "x"), "\x1b[38;5;255mx\x1b[0m");
    }
}
