//! Terminal plumbing: raw mode, frame output and the event loop.
//!
//! The UI is drawn on **stderr** so stdout stays free for the script the shell
//! wrapper evaluates.

use super::keys::Key;
use super::selector::{Outcome, Screen, Selector};
use crossterm::event::{self, Event};
use crossterm::{cursor, execute, terminal};
use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};

/// Test hooks (`--and-keys`, `--and-exit`, `--and-confirm`).
#[derive(Debug, Default)]
pub struct Hooks {
    pub keys: Vec<Key>,
    pub and_exit: bool,
    pub confirm: Option<String>,
}

impl Hooks {
    fn is_scripted(&self) -> bool {
        !self.keys.is_empty() || self.and_exit
    }
}

/// `(columns, rows)`; `TRY_WIDTH` / `TRY_HEIGHT` override the real terminal.
pub fn size() -> (usize, usize) {
    let env = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
    };
    let real = terminal::size().ok();
    (
        env("TRY_WIDTH")
            .or(real.map(|s| s.0 as usize))
            .unwrap_or(80),
        env("TRY_HEIGHT")
            .or(real.map(|s| s.1 as usize))
            .unwrap_or(24),
    )
}

/// Build one full-screen frame. With `pin_footer` the footer sits on the last rows.
pub fn compose(screen: &Screen, height: usize, interactive: bool) -> String {
    let nl = if interactive { "\r\n" } else { "\n" };
    let room = height.saturating_sub(screen.header.len() + screen.footer.len());
    let body = &screen.body[..screen.body.len().min(room)];

    let mut lines: Vec<&str> = screen.header.iter().map(String::as_str).collect();
    lines.extend(body.iter().map(String::as_str));
    if interactive {
        let blanks = height.saturating_sub(lines.len() + screen.footer.len());
        lines.extend(std::iter::repeat_n("", blanks));
    }
    lines.extend(screen.footer.iter().map(String::as_str));

    let mut out = String::new();
    if interactive {
        out.push_str("\x1b[?25l");
    }
    out.push_str("\x1b[H");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(nl);
        }
        out.push_str(line);
        out.push_str("\x1b[K");
    }
    if !interactive {
        out.push('\n');
    }
    out.push_str("\x1b[J");
    out
}

fn draw(sel: &mut Selector, interactive: bool) -> io::Result<()> {
    let (w, h) = size();
    let frame = compose(&sel.render(w, h), h, interactive);
    let mut err = io::stderr().lock();
    err.write_all(frame.as_bytes())?;
    err.flush()
}

/// Run the selector to completion.
pub fn run(sel: &mut Selector, script: Hooks) -> io::Result<Outcome> {
    if script.is_scripted() {
        return run_scripted(sel, script);
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(io::Error::other("try requires an interactive terminal"));
    }
    run_interactive(sel)
}

/// Replay injected keys without touching terminal modes.
fn run_scripted(sel: &mut Selector, script: Hooks) -> io::Result<Outcome> {
    let mut keys: VecDeque<Key> = script.keys.into();
    let mut confirm = script.confirm;
    loop {
        let (_, h) = size();
        sel.set_height(h);
        if !script.and_exit {
            draw(sel, false)?;
        }
        let Some(key) = keys.pop_front() else {
            if script.and_exit {
                draw(sel, false)?;
            }
            return Ok(Outcome::Cancel);
        };
        if let Some(done) = sel.handle_key(key) {
            return Ok(done);
        }
        if sel.awaiting_delete_confirmation() {
            if let Some(text) = confirm.take() {
                for c in text.chars() {
                    sel.handle_key(Key::Char(c));
                }
                if let Some(done) = sel.handle_key(Key::Enter) {
                    return Ok(done);
                }
            }
        }
    }
}

/// Restores the terminal on every exit path, including panics.
struct RawGuard;

impl RawGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = RawGuard;
        execute!(io::stderr(), terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn run_interactive(sel: &mut Selector) -> io::Result<Outcome> {
    let _guard = RawGuard::enter()?;
    loop {
        draw(sel, true)?;
        // Resize (and any other event) just triggers a redraw.
        if let Event::Key(ev) = event::read()? {
            if let Some(done) = Key::from_event(ev).and_then(|k| sel.handle_key(k)) {
                return Ok(done);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pins_footer_and_clips_body() {
        let screen = Screen {
            header: vec!["H1".into(), "H2".into()],
            body: (0..10).map(|i| format!("B{i}")).collect(),
            footer: vec!["F1".into(), "F2".into()],
        };
        let plain = compose(&screen, 8, false);
        assert!(plain.contains("B3") && !plain.contains("B4"));
        let tty = compose(&screen, 12, true);
        let rows: Vec<&str> = tty.split("\r\n").collect();
        assert_eq!(rows.len(), 12);
        assert!(rows[10].starts_with("F1") && rows[11].starts_with("F2"));
    }
}
