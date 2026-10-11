//! A single-line text field with readline-style editing.

use super::keys::Key;
use super::style::{char_width, Style};

#[derive(Debug, Clone, Default)]
pub struct InputField {
    chars: Vec<char>,
    cursor: usize,
}

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

impl InputField {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        InputField {
            cursor: chars.len(),
            chars,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Apply an editing key. Returns whether the key was consumed.
    pub fn handle(&mut self, key: Key) -> bool {
        match key {
            Key::Char(c) if !c.is_control() => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            Key::Backspace | Key::Ctrl('h') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            Key::Delete => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            Key::Home | Key::Ctrl('a') => self.cursor = 0,
            Key::End | Key::Ctrl('e') => self.cursor = self.chars.len(),
            Key::Left | Key::Ctrl('b') => self.cursor = self.cursor.saturating_sub(1),
            Key::Right | Key::Ctrl('f') => self.cursor = (self.cursor + 1).min(self.chars.len()),
            Key::Ctrl('k') => self.chars.truncate(self.cursor),
            Key::Ctrl('u') => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            Key::Ctrl('w') => {
                let mut p = self.cursor;
                while p > 0 && !alnum(self.chars[p - 1]) {
                    p -= 1;
                }
                while p > 0 && alnum(self.chars[p - 1]) {
                    p -= 1;
                }
                self.chars.drain(p..self.cursor);
                self.cursor = p;
            }
            _ => return false,
        }
        true
    }

    /// Text with the cursor drawn as a reverse-video cell.
    pub fn render(&self, style: Style) -> String {
        let before: String = self.chars[..self.cursor].iter().collect();
        let at = self.chars.get(self.cursor).copied().unwrap_or(' ');
        let after: String = self
            .chars
            .get(self.cursor + 1..)
            .unwrap_or(&[])
            .iter()
            .collect();
        format!("{before}{}{after}", style.cursor(&at.to_string()))
    }

    /// Columns taken by [`render`](Self::render).
    pub fn visible_width(&self) -> usize {
        let text: usize = self.chars.iter().map(|&c| char_width(c)).sum();
        text + usize::from(self.cursor >= self.chars.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> InputField {
        let mut f = InputField::default();
        for c in s.chars() {
            f.handle(Key::Char(c));
        }
        f
    }

    #[test]
    fn insert_and_move() {
        let mut f = typed("helo");
        f.handle(Key::Left);
        f.handle(Key::Char('l'));
        assert_eq!(f.text(), "hello");
        f.handle(Key::Home);
        f.handle(Key::Delete);
        assert_eq!(f.text(), "ello");
        f.handle(Key::End);
        f.handle(Key::Backspace);
        assert_eq!(f.text(), "ell");
    }

    #[test]
    fn kill_commands() {
        let mut f = typed("foo-bar baz");
        f.handle(Key::Ctrl('w'));
        assert_eq!(f.text(), "foo-bar ");
        f.handle(Key::Ctrl('w'));
        assert_eq!(f.text(), "foo-");
        f.handle(Key::Ctrl('u'));
        assert_eq!(f.text(), "");

        let mut f = typed("abcdef");
        f.handle(Key::Left);
        f.handle(Key::Left);
        f.handle(Key::Ctrl('k'));
        assert_eq!(f.text(), "abcd");
        f.handle(Key::Ctrl('a'));
        f.handle(Key::Ctrl('u'));
        assert_eq!(f.text(), "abcd");
    }

    #[test]
    fn non_editing_keys_are_not_consumed() {
        let mut f = typed("x");
        assert!(!f.handle(Key::Enter));
        assert!(!f.handle(Key::Esc));
        assert!(!f.handle(Key::Up));
        assert!(!f.handle(Key::Ctrl('d')));
        assert_eq!(f.text(), "x");
    }

    #[test]
    fn unicode_editing() {
        let mut f = typed("é📁x");
        f.handle(Key::Backspace);
        f.handle(Key::Backspace);
        assert_eq!(f.text(), "é");
        assert_eq!(f.visible_width(), 2);
    }
}
