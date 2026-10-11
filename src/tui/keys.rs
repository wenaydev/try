//! Terminal-independent key representation and the `--and-keys` mini-language.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Ctrl + lowercase letter.
    Ctrl(char),
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    Tab,
}

impl Key {
    /// Convert a crossterm event; `None` for releases and keys we ignore.
    pub fn from_event(ev: KeyEvent) -> Option<Key> {
        if ev.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        Some(match ev.code {
            KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
            KeyCode::Char(_) if alt => return None,
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Tab => Key::Tab,
            _ => return None,
        })
    }
}

fn symbolic(tok: &str) -> Option<Key> {
    let upper = tok.to_ascii_uppercase();
    if let Some(rest) = upper
        .strip_prefix("CTRL-")
        .or_else(|| upper.strip_prefix("CTRL"))
    {
        let mut it = rest.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            if c.is_ascii_alphabetic() {
                return Some(Key::Ctrl(c.to_ascii_lowercase()));
            }
        }
    }
    // Names are only symbolic when written in upper case; `up` is typed text.
    if tok != upper {
        return None;
    }
    Some(match upper.as_str() {
        "ENTER" | "RETURN" => Key::Enter,
        "ESC" | "ESCAPE" => Key::Esc,
        "UP" => Key::Up,
        "DOWN" => Key::Down,
        "LEFT" => Key::Left,
        "RIGHT" => Key::Right,
        "HOME" => Key::Home,
        "END" => Key::End,
        "BACKSPACE" | "BS" => Key::Backspace,
        "DELETE" | "DEL" => Key::Delete,
        "TAB" => Key::Tab,
        "SPACE" => Key::Char(' '),
        _ => return None,
    })
}

/// Parse an `--and-keys` specification.
///
/// *Token mode* (the spec contains a comma, or is only `A-Z` and `-`):
/// comma-separated names such as `DOWN,ENTER`, `CTRL-N`, `TYPE=text`; any other
/// token is typed literally.
///
/// *Raw mode*: every character is a key, with `ESC [ A`-style sequences, `\r`,
/// `\x7f` and control bytes decoded.
pub fn parse_spec(spec: &str) -> Vec<Key> {
    let token_mode = spec.contains(',') || spec.chars().all(|c| c.is_ascii_uppercase() || c == '-');
    if token_mode {
        let mut keys = Vec::new();
        for tok in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if let Some(text) = tok
                .strip_prefix("TYPE=")
                .or_else(|| tok.strip_prefix("type="))
            {
                keys.extend(text.chars().map(Key::Char));
            } else if let Some(k) = symbolic(tok) {
                keys.push(k);
            } else {
                keys.extend(tok.chars().map(Key::Char));
            }
        }
        return keys;
    }

    let chars: Vec<char> = spec.chars().collect();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        keys.push(match c {
            '\x1b' if chars.get(i) == Some(&'[') => {
                // CSI: parameters then a final byte in 0x40..=0x7e.
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && !('\x40'..='\x7e').contains(&chars[j]) {
                    j += 1;
                }
                let Some(&fin) = chars.get(j) else { break };
                let params: String = chars[start..j].iter().collect();
                i = j + 1;
                match (params.as_str(), fin) {
                    (_, 'A') => Key::Up,
                    (_, 'B') => Key::Down,
                    (_, 'C') => Key::Right,
                    (_, 'D') => Key::Left,
                    (_, 'H') => Key::Home,
                    (_, 'F') => Key::End,
                    ("3", '~') => Key::Delete,
                    ("1" | "7", '~') => Key::Home,
                    ("4" | "8", '~') => Key::End,
                    _ => continue,
                }
            }
            '\x1b' => Key::Esc,
            '\r' => Key::Enter,
            '\t' => Key::Tab,
            '\x7f' | '\x08' => Key::Backspace,
            '\x01'..='\x1a' => Key::Ctrl((b'a' + (c as u8 - 1)) as char),
            c if !c.is_control() => Key::Char(c),
            _ => continue,
        });
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_mode() {
        assert_eq!(
            parse_spec("beta,ENTER"),
            vec![
                Key::Char('b'),
                Key::Char('e'),
                Key::Char('t'),
                Key::Char('a'),
                Key::Enter
            ]
        );
        assert_eq!(
            parse_spec("CTRL-J,CTRL-K,ENTER"),
            vec![Key::Ctrl('j'), Key::Ctrl('k'), Key::Enter]
        );
        assert_eq!(parse_spec("ESC"), vec![Key::Esc]);
        assert_eq!(parse_spec("CTRL-D,TYPE=YES,ENTER")[1], Key::Char('Y'));
        assert_eq!(
            parse_spec("up,UP"),
            vec![Key::Char('u'), Key::Char('p'), Key::Up]
        );
    }

    #[test]
    fn raw_mode() {
        assert_eq!(
            parse_spec("ab\x1b[B\x1b[A\x7f\r"),
            vec![
                Key::Char('a'),
                Key::Char('b'),
                Key::Down,
                Key::Up,
                Key::Backspace,
                Key::Enter
            ]
        );
        assert_eq!(parse_spec("\x1b"), vec![Key::Esc]);
        assert_eq!(
            parse_spec("\x0a\x0b\r"),
            vec![Key::Ctrl('j'), Key::Ctrl('k'), Key::Enter]
        );
        assert_eq!(parse_spec("\x1b[3~"), vec![Key::Delete]);
    }
}
