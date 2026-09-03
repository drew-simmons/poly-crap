//! Terminal colors for human output.
//!
//! A [`Theme`] decides whether a style renders. Everything printed for a
//! person takes one, and the machine formats never see one, so JSON and SARIF
//! cannot pick up an escape code by accident.

use anstyle::{AnsiColor, Effects, Style};
use clap::ValueEnum;
use std::fmt::Display;

/// When to color output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    /// Color a terminal that supports it, and honor `NO_COLOR`.
    #[default]
    Auto,
    /// Write escape codes even to a pipe or a file.
    Always,
    /// Never write escape codes.
    Never,
}

/// Bold, for headers.
pub const HEADER: Style = Style::new().effects(Effects::BOLD);
/// Dim, for lines that give context rather than findings.
pub const DIM: Style = Style::new().effects(Effects::DIMMED);
/// Red bold, for a failure.
pub const BAD: Style = AnsiColor::Red.on_default().effects(Effects::BOLD);
/// Yellow, for a value close to failing or a warning.
pub const WARN: Style = AnsiColor::Yellow.on_default();
/// Green, for a value that passes.
pub const GOOD: Style = AnsiColor::Green.on_default();
/// Cyan, for a note or a new function.
pub const NOTE: Style = AnsiColor::Cyan.on_default();
/// Magenta, for a moved function.
pub const MOVED: Style = AnsiColor::Magenta.on_default();

/// Whether styles render. Copy, so it passes by value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Theme {
    enabled: bool,
}

impl Theme {
    /// Render every style as plain text.
    #[must_use]
    pub const fn plain() -> Self {
        Self { enabled: false }
    }

    /// Render ANSI escape codes.
    #[must_use]
    pub const fn ansi() -> Self {
        Self { enabled: true }
    }

    /// Resolve a mode. `always` and `never` decide on their own; `auto` asks
    /// the closure, which is where the stream check lives.
    pub fn resolve(mode: ColorMode, auto: impl FnOnce() -> bool) -> Self {
        match mode {
            ColorMode::Always => Self::ansi(),
            ColorMode::Never => Self::plain(),
            ColorMode::Auto => Self { enabled: auto() },
        }
    }

    /// Wrap text in a style, or return it as is when styles are off. Empty
    /// text is returned bare either way, so a blank cell leaves no code behind.
    #[must_use]
    pub fn paint(self, style: Style, text: impl Display) -> String {
        let text = text.to_string();
        if self.enabled && !text.is_empty() {
            format!("{style}{text}{style:#}")
        } else {
            text
        }
    }
}

/// True when a stream should be colored under `auto`: a terminal that supports
/// color, or `CLICOLOR_FORCE`, and never under `NO_COLOR` or `TERM=dumb`.
pub fn stream_wants_color(stream: &impl anstream::stream::RawStream) -> bool {
    anstream::AutoStream::choice(stream) != anstream::ColorChoice::Never
}

/// A count with its noun, pluralized: `1 function`, `2 functions`.
#[must_use]
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A `warning:` line for stderr.
#[must_use]
pub fn warning(theme: Theme, message: impl Display) -> String {
    prefixed(theme, WARN.effects(Effects::BOLD), "warning", message)
}

/// A `note:` line for stderr.
#[must_use]
pub fn note(theme: Theme, message: impl Display) -> String {
    prefixed(theme, NOTE.effects(Effects::BOLD), "note", message)
}

/// An `error:` line for stderr.
#[must_use]
pub fn error(theme: Theme, message: impl Display) -> String {
    prefixed(theme, BAD, "error", message)
}

fn prefixed(theme: Theme, style: Style, prefix: &str, message: impl Display) -> String {
    format!("{} {message}", theme.paint(style, format!("{prefix}:")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_theme_leaves_text_alone() {
        assert_eq!(Theme::plain().paint(BAD, "x"), "x");
        assert_eq!(Theme::default(), Theme::plain());
    }

    #[test]
    fn ansi_theme_wraps_and_resets() {
        let painted = Theme::ansi().paint(BAD, "x");
        assert!(painted.starts_with("\x1b["), "{painted:?}");
        assert!(painted.ends_with("x\x1b[0m"), "{painted:?}");
        // An empty style has nothing to start or reset.
        assert_eq!(Theme::ansi().paint(Style::new(), "x"), "x");
        // Empty text gets no codes, so a blank cell stays blank.
        assert_eq!(Theme::ansi().paint(BAD, ""), "");
    }

    #[test]
    fn resolve_asks_the_stream_only_under_auto() {
        assert_eq!(Theme::resolve(ColorMode::Always, || false), Theme::ansi());
        assert_eq!(Theme::resolve(ColorMode::Never, || true), Theme::plain());
        assert_eq!(Theme::resolve(ColorMode::Auto, || true), Theme::ansi());
        assert_eq!(Theme::resolve(ColorMode::Auto, || false), Theme::plain());
    }

    #[test]
    fn count_pluralizes_regular_nouns() {
        assert_eq!(count(0, "function"), "0 functions");
        assert_eq!(count(1, "function"), "1 function");
        assert_eq!(count(2, "changed file"), "2 changed files");
    }

    #[test]
    fn stderr_prefixes_are_painted_and_the_message_is_not() {
        assert_eq!(warning(Theme::plain(), "stale"), "warning: stale");
        assert_eq!(note(Theme::plain(), "found"), "note: found");
        assert_eq!(error(Theme::plain(), "bad"), "error: bad");
        let colored = warning(Theme::ansi(), "stale");
        assert!(colored.starts_with("\x1b["), "{colored:?}");
        assert!(colored.ends_with("warning:\x1b[0m stale"), "{colored:?}");
    }
}
