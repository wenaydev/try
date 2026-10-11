//! ANSI styling and display-width helpers.
//!
//! Styled spans reset only the attribute they set (foreground/intensity) so
//! they nest inside a row background without cutting it short.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const RESET: &str = "\x1b[0m";
const FG_OFF: &str = "\x1b[39m";
const FG_BOLD_OFF: &str = "\x1b[39m\x1b[22m";

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn wrap(&self, text: &str, on: &str, off: &str) -> String {
        if self.color {
            format!("{on}{text}{off}")
        } else {
            text.to_string()
        }
    }

    pub fn dim(&self, t: &str) -> String {
        self.wrap(t, "\x1b[38;5;245m", FG_OFF)
    }
    pub fn accent(&self, t: &str) -> String {
        self.wrap(t, "\x1b[1;38;5;214m", FG_BOLD_OFF)
    }
    pub fn highlight(&self, t: &str) -> String {
        self.wrap(t, "\x1b[1;33m", FG_BOLD_OFF)
    }
    pub fn good(&self, t: &str) -> String {
        self.wrap(t, "\x1b[32m", FG_OFF)
    }
    pub fn warn(&self, t: &str) -> String {
        self.wrap(t, "\x1b[33m", FG_OFF)
    }
    pub fn bold(&self, t: &str) -> String {
        self.wrap(t, "\x1b[1m", "\x1b[22m")
    }
    /// Selected-row background + readable foreground.
    pub fn selected(&self) -> &'static str {
        if self.color {
            "\x1b[48;5;238m\x1b[38;5;255m"
        } else {
            ""
        }
    }
    pub fn sel_fg(&self) -> &'static str {
        if self.color {
            "\x1b[38;5;255m"
        } else {
            ""
        }
    }
    pub fn danger_bg(&self) -> &'static str {
        if self.color {
            "\x1b[48;5;52m"
        } else {
            ""
        }
    }
    /// Reverse video, kept even without colors so the cursor stays visible.
    pub fn cursor(&self, t: &str) -> String {
        format!("\x1b[7m{t}\x1b[27m")
    }
}

/// Terminal columns occupied by `s` (no ANSI codes expected).
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Longest prefix of `s` that fits in `cols` columns.
pub fn clip(s: &str, cols: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        let w = char_width(c);
        if used + w > cols {
            return &s[..i];
        }
        used += w;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_clipping() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("📁"), 2);
        assert_eq!(clip("abcdef", 3), "abc");
        assert_eq!(clip("a📁b", 2), "a");
        assert_eq!(clip("abc", 10), "abc");
    }

    #[test]
    fn no_color_emits_no_sgr_except_cursor() {
        let s = Style { color: false };
        assert_eq!(s.dim("x"), "x");
        assert_eq!(s.highlight("x"), "x");
        assert_eq!(s.selected(), "");
        assert!(s.cursor("x").contains("\x1b[7m"));
    }
}
