//! The selector: state machine and rendering, free of any terminal I/O.
//!
//! [`Selector::handle_key`] consumes keys and eventually yields an
//! [`Outcome`]; [`Selector::render`] turns the current state into text lines.
//! `term` wires both to a real (or scripted) terminal.

use super::input::InputField;
use super::keys::Key;
use super::style::{clip, width, Style, RESET};
use crate::entries::{self, Entry};
use crate::fuzzy::{Fuzzy, Match};
use crate::gitstatus::{self, GitStatus, Probe};
use crate::naming;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Cancel,
    /// Visit an existing directory.
    Cd(PathBuf),
    /// Create then visit.
    Mkdir(PathBuf),
    Rename {
        old: String,
        new: String,
    },
    Graduate {
        source: PathBuf,
        dest: PathBuf,
        name: String,
    },
    /// `names` are basenames directly under the canonical `base`.
    Delete {
        base: PathBuf,
        names: Vec<String>,
    },
}

/// Rendered text, split so the caller can pin the footer to the screen bottom.
#[derive(Debug, Default)]
pub struct Screen {
    pub header: Vec<String>,
    pub body: Vec<String>,
    pub footer: Vec<String>,
}

#[derive(Debug)]
struct Dialog {
    input: InputField,
    error: Option<String>,
}

impl Dialog {
    fn new(text: &str) -> Self {
        Dialog {
            input: InputField::new(text),
            error: None,
        }
    }
}

#[derive(Debug)]
enum Kind {
    Rename {
        name: String,
    },
    Graduate {
        name: String,
        source: PathBuf,
    },
    /// `warnings`: unsaved git work among the marked directories.
    ConfirmDelete {
        warnings: Vec<String>,
    },
    NewName,
}

#[derive(Debug)]
enum Mode {
    Browse,
    Dialog(Kind, Dialog),
}

const HELP: &str =
    "↑/↓: Navigate  Enter: Select  ^R: Rename  ^G: Graduate  ^D: Delete  Esc: Cancel";

pub struct Selector {
    base: PathBuf,
    projects_dir: PathBuf,
    entries: Vec<Entry>,
    fuzzy: Fuzzy,
    matches: Vec<Match>,
    input: InputField,
    cursor: usize,
    scroll: usize,
    /// Maximum number of matches shown; follows the terminal height.
    limit: usize,
    mode: Mode,
    marked: Vec<String>,
    status: Option<String>,
    style: Style,
    today: String,
    now: SystemTime,
    /// Whether confirmed deletes go to the trash (only affects the dialog text).
    trash: bool,
    git_enabled: bool,
    git: HashMap<String, Probe>,
    git_requested: HashSet<String>,
    /// Range of `matches` indices drawn by the last browse render.
    visible: std::ops::Range<usize>,
}

impl Selector {
    /// Load `base` (creating it if missing) and start with `query` typed.
    pub fn open(base: &Path, query: &str, projects_dir: PathBuf, style: Style) -> io::Result<Self> {
        fs::create_dir_all(base)?;
        let entries = entries::load(base)?;
        Ok(Self::with_entries(
            base.to_path_buf(),
            entries,
            query,
            projects_dir,
            style,
            naming::today(),
            SystemTime::now(),
        ))
    }

    pub fn with_entries(
        base: PathBuf,
        entries: Vec<Entry>,
        query: &str,
        projects_dir: PathBuf,
        style: Style,
        today: String,
        now: SystemTime,
    ) -> Self {
        let fuzzy = Fuzzy::new(entries.iter().map(|e| (e.name.as_str(), e.base_score)));
        let mut s = Selector {
            base,
            projects_dir,
            entries,
            fuzzy,
            matches: Vec::new(),
            input: InputField::new(&naming::dashify(query)),
            cursor: 0,
            scroll: 0,
            limit: 10,
            mode: Mode::Browse,
            marked: Vec::new(),
            status: None,
            style,
            today,
            now,
            trash: false,
            git_enabled: false,
            git: HashMap::new(),
            git_requested: HashSet::new(),
            visible: 0..0,
        };
        s.refilter();
        s
    }

    /// Turn list markers and delete-time git warnings on or off.
    pub fn set_git_status(&mut self, enabled: bool) {
        self.git_enabled = enabled;
    }

    pub fn git_enabled(&self) -> bool {
        self.git_enabled
    }

    /// Directories on screen whose git state is still unknown. Each is handed
    /// out once; call after rendering.
    pub fn take_git_requests(&mut self) -> Vec<(String, PathBuf)> {
        if !self.git_enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in self.visible.clone() {
            let Some(m) = self.matches.get(i) else {
                continue;
            };
            let e = &self.entries[m.index];
            if self.git_requested.insert(e.name.clone()) {
                out.push((e.name.clone(), e.path.clone()));
            }
        }
        out
    }

    pub fn set_git(&mut self, name: String, probe: Probe) {
        self.git.insert(name, probe);
    }

    /// Requested probes that have not reported back yet.
    pub fn git_pending(&self) -> bool {
        self.git_requested.len() > self.git.len()
    }

    /// Plain-language warnings for marked directories holding unsaved git work.
    fn delete_warnings(&mut self) -> Vec<String> {
        if !self.git_enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for name in self.marked.clone() {
            let probe = match self.git.get(&name) {
                Some(p @ (Probe::Repo(_) | Probe::NotRepo)) => *p,
                _ => {
                    let p = gitstatus::probe(&self.base.join(&name));
                    self.git.insert(name.clone(), p);
                    p
                }
            };
            let Probe::Repo(g) = probe else { continue };
            let mut issues = Vec::new();
            if g.dirty {
                issues.push("uncommitted changes".to_string());
            }
            if g.ahead > 0 {
                issues.push(format!("{} unpushed commit(s)", g.ahead));
            }
            if !g.has_upstream && !self.trash {
                issues.push("no upstream, commits may exist only here".to_string());
            }
            if !issues.is_empty() {
                out.push(format!("{name}: {}", issues.join(", ")));
            }
        }
        out
    }

    fn git_marker(&self, name: &str) -> Option<(String, usize)> {
        let st = self.style;
        match self.git.get(name)? {
            Probe::NotRepo => None,
            Probe::Failed => Some((st.dim("?"), 1)),
            Probe::Repo(g) => Some(marker(&st, g)),
        }
    }

    /// Describe deletion as recoverable (trash) rather than permanent.
    pub fn set_trash(&mut self, trash: bool) {
        self.trash = trash;
    }

    /// Tell the selector how tall the terminal is (bounds the result list).
    pub fn set_height(&mut self, height: usize) {
        self.limit = height.saturating_sub(6).max(3);
        self.cursor = self.cursor.min(self.total().saturating_sub(1));
    }

    pub fn query(&self) -> String {
        self.input.text()
    }

    /// True while the "type YES" confirmation is on screen.
    pub fn awaiting_delete_confirmation(&self) -> bool {
        matches!(self.mode, Mode::Dialog(Kind::ConfirmDelete { .. }, _))
    }

    fn refilter(&mut self) {
        self.matches = self.fuzzy.matches(&self.input.text());
    }

    fn shown(&self) -> usize {
        self.matches.len().min(self.limit)
    }

    fn has_create(&self) -> bool {
        !self.input.is_empty()
    }

    fn total(&self) -> usize {
        self.shown() + usize::from(self.has_create())
    }

    fn entry_at(&self, idx: usize) -> &Entry {
        &self.entries[self.matches[idx].index]
    }

    // ---------------------------------------------------------------- input

    pub fn handle_key(&mut self, key: Key) -> Option<Outcome> {
        match self.mode {
            Mode::Browse => self.browse_key(key),
            Mode::Dialog(..) => self.dialog_key(key),
        }
    }

    fn browse_key(&mut self, key: Key) -> Option<Outcome> {
        let total = self.total();
        let shown = self.shown();
        match key {
            Key::Up | Key::Ctrl('p') | Key::Ctrl('k') => {
                self.cursor = self.cursor.saturating_sub(1)
            }
            Key::Down | Key::Ctrl('n') | Key::Ctrl('j') => {
                if self.cursor + 1 < total {
                    self.cursor += 1;
                }
            }
            Key::Enter => {
                if !self.marked.is_empty() {
                    let warnings = self.delete_warnings();
                    self.mode = Mode::Dialog(Kind::ConfirmDelete { warnings }, Dialog::new(""));
                } else if self.cursor < shown {
                    return Some(Outcome::Cd(self.entry_at(self.cursor).path.clone()));
                } else if self.has_create() {
                    return self.create_new();
                }
            }
            Key::Ctrl('d') => {
                if self.cursor < shown {
                    let name = self.entry_at(self.cursor).name.clone();
                    match self.marked.iter().position(|m| *m == name) {
                        Some(i) => {
                            self.marked.remove(i);
                        }
                        None => self.marked.push(name),
                    }
                }
            }
            Key::Ctrl('t') => return self.create_new(),
            Key::Ctrl('r') if self.cursor < shown => {
                self.marked.clear();
                let name = self.entry_at(self.cursor).name.clone();
                let d = Dialog::new(&name);
                self.mode = Mode::Dialog(Kind::Rename { name }, d);
            }
            Key::Ctrl('g') if self.cursor < shown => {
                self.marked.clear();
                let e = self.entry_at(self.cursor);
                if e.is_symlink {
                    self.status = Some(format!("{} is already a symlink (graduated?)", e.name));
                } else {
                    let (name, source) = (e.name.clone(), e.path.clone());
                    let dest = self.projects_dir.join(naming::strip_date_prefix(&name));
                    let d = Dialog::new(&dest.to_string_lossy());
                    self.mode = Mode::Dialog(Kind::Graduate { name, source }, d);
                }
            }
            Key::Esc | Key::Ctrl('c') => {
                if self.marked.is_empty() {
                    return Some(Outcome::Cancel);
                }
                self.marked.clear();
            }
            other => {
                let before = self.input.text();
                if self.input.handle(other) && self.input.text() != before {
                    self.cursor = 0;
                    self.refilter();
                }
            }
        }
        None
    }

    fn create_new(&mut self) -> Option<Outcome> {
        if self.input.is_empty() {
            self.marked.clear();
            self.mode = Mode::Dialog(Kind::NewName, Dialog::new(""));
            return None;
        }
        Some(self.mkdir_outcome(&self.input.text()))
    }

    fn mkdir_outcome(&self, name: &str) -> Outcome {
        let full = format!("{}-{}", self.today, naming::sanitize_name(name));
        Outcome::Mkdir(self.base.join(full))
    }

    fn dialog_key(&mut self, key: Key) -> Option<Outcome> {
        let Mode::Dialog(kind, mut d) = std::mem::replace(&mut self.mode, Mode::Browse) else {
            return None;
        };
        match key {
            Key::Esc | Key::Ctrl('c') => {
                if matches!(kind, Kind::ConfirmDelete { .. }) {
                    self.status = Some("Delete cancelled".into());
                    self.marked.clear();
                }
                None
            }
            Key::Enter => match self.submit(&kind, &d.input.text()) {
                Ok(outcome) => outcome,
                Err(msg) => {
                    d.error = Some(msg);
                    self.mode = Mode::Dialog(kind, d);
                    None
                }
            },
            other => {
                let before = d.input.text();
                d.input.handle(other);
                if d.input.text() != before {
                    d.error = None;
                }
                self.mode = Mode::Dialog(kind, d);
                None
            }
        }
    }

    /// `Ok(None)` closes the dialog without a result; `Err` keeps it open.
    fn submit(&mut self, kind: &Kind, text: &str) -> Result<Option<Outcome>, String> {
        match kind {
            Kind::NewName => {
                if text.trim().is_empty() {
                    return Ok(None);
                }
                Ok(Some(self.mkdir_outcome(text)))
            }
            Kind::Rename { name: old } => {
                let new = naming::dashify(text);
                if new.is_empty() {
                    return Err("Name cannot be empty".into());
                }
                if new.contains('/') {
                    return Err("Name cannot contain /".into());
                }
                if new == "." || new == ".." {
                    return Err("Invalid name".into());
                }
                if new == *old {
                    return Ok(None);
                }
                if fs::symlink_metadata(self.base.join(&new)).is_ok() {
                    return Err(format!("Directory exists: {new}"));
                }
                Ok(Some(Outcome::Rename {
                    old: old.clone(),
                    new,
                }))
            }
            Kind::Graduate { name, source } => {
                let dest = expand_dest(text.trim())?;
                if fs::symlink_metadata(&dest).is_ok() {
                    return Err(format!("Destination already exists: {}", dest.display()));
                }
                let parent = dest.parent().unwrap_or(Path::new("/"));
                if !parent.is_dir() {
                    return Err(format!(
                        "Parent directory does not exist: {}",
                        parent.display()
                    ));
                }
                Ok(Some(Outcome::Graduate {
                    source: source.clone(),
                    dest,
                    name: name.clone(),
                }))
            }
            Kind::ConfirmDelete { .. } => {
                if text != "YES" {
                    self.status = Some("Delete cancelled".into());
                    self.marked.clear();
                    return Ok(None);
                }
                match self.validate_delete() {
                    Ok(base) => Ok(Some(Outcome::Delete {
                        base,
                        names: std::mem::take(&mut self.marked),
                    })),
                    Err(e) => {
                        self.status = Some(format!("Error: {e}"));
                        Ok(None)
                    }
                }
            }
        }
    }

    /// Every marked entry must resolve to a location strictly inside the
    /// tries root (this is what stops a symlink from redirecting `rm -rf`).
    fn validate_delete(&self) -> io::Result<PathBuf> {
        let base = fs::canonicalize(&self.base)?;
        for name in &self.marked {
            let target = fs::canonicalize(self.base.join(name))?;
            if target == base || !target.starts_with(&base) {
                return Err(io::Error::other(format!(
                    "Safety check failed: {} is not inside {}",
                    target.display(),
                    base.display()
                )));
            }
        }
        Ok(base)
    }

    // -------------------------------------------------------------- render

    pub fn render(&mut self, w: usize, h: usize) -> Screen {
        self.set_height(h);
        match &self.mode {
            Mode::Browse => self.render_browse(w, h),
            Mode::Dialog(kind, d) => self.render_dialog(kind, d, w),
        }
    }

    fn sep(&self, w: usize) -> String {
        self.style.dim(&"─".repeat(w.saturating_sub(1)))
    }

    fn render_browse(&mut self, w: usize, h: usize) -> Screen {
        let st = self.style;
        let header = vec![
            format!("🏠{}", st.accent(" Try Directory Selection")),
            self.sep(w),
            format!("{}{}", st.dim("Search: "), self.input.render(st)),
            self.sep(w),
        ];

        let mut footer = vec![self.sep(w)];
        if let Some(status) = self.status.take() {
            footer.push(st.bold(&status));
        } else if !self.marked.is_empty() {
            let text = format!(
                " {} marked  |  Ctrl-D: Toggle  Enter: Confirm  Esc: Cancel",
                self.marked.len()
            );
            let label = " DELETE MODE ";
            let banner = format!("{}{}{}", st.danger_bg(), st.bold(label), text);
            footer.push(pad_bg(
                &banner,
                width(label) + width(&text),
                w,
                st.danger_bg(),
            ));
        } else {
            let help = clip(HELP, w.saturating_sub(1));
            footer.push(centered(&st.dim(help), width(help), w));
        }

        let rows = h.saturating_sub(header.len() + footer.len()).max(3);
        let shown = self.shown();
        let total = self.total();
        let row_cost = |idx: usize| 1 + usize::from(idx == shown && shown > 0);

        // Scroll so the cursor row is fully visible.
        self.scroll = self.scroll.min(self.cursor);
        while (self.scroll..=self.cursor).map(row_cost).sum::<usize>() > rows {
            self.scroll += 1;
        }

        let mut body = Vec::new();
        let mut last_entry = self.scroll;
        for idx in self.scroll..total {
            if body.len() + row_cost(idx) > rows {
                break;
            }
            if idx == shown && shown > 0 {
                body.push(String::new());
            }
            body.push(if idx < shown {
                last_entry = idx + 1;
                self.entry_line(idx, idx == self.cursor, w)
            } else {
                self.create_line(idx == self.cursor, w)
            });
        }
        self.visible = self.scroll..last_entry.min(shown);
        Screen {
            header,
            body,
            footer,
        }
    }

    fn arrow(&self, selected: bool) -> String {
        if selected {
            format!("{}{}", self.style.highlight("→ "), self.style.sel_fg())
        } else {
            "  ".into()
        }
    }

    fn entry_line(&self, idx: usize, selected: bool, w: usize) -> String {
        let st = self.style;
        let m = &self.matches[idx];
        let e = self.entry_at(idx);
        let marked = self.marked.contains(&e.name);

        let bg = if marked {
            format!(
                "{}{}",
                st.danger_bg(),
                if selected { st.sel_fg() } else { "" }
            )
        } else if selected {
            st.selected().to_string()
        } else {
            String::new()
        };
        let icon = if marked {
            "🗑️"
        } else if e.is_symlink {
            "🔗"
        } else {
            "📁"
        };

        // Only truncate when the name would run off the line, not to make room
        // for the metadata.
        let prefix_w = 5;
        let max_name = w.saturating_sub(prefix_w + 1);
        let name_w = width(&e.name);
        let (name, truncated) = if name_w > max_name && max_name > 2 {
            (clip(&e.name, max_name - 1), true)
        } else {
            (e.name.as_str(), false)
        };

        let dated = naming::has_date_prefix(&e.name) && e.name.chars().count() > 11;
        let mut rendered = String::new();
        let mut run = String::new();
        let mut run_kind = Span::Plain;
        for (i, c) in name.chars().enumerate() {
            let kind = if m.positions.binary_search(&i).is_ok() {
                Span::Match
            } else if dated && i < 11 && !selected {
                Span::Date
            } else {
                Span::Plain
            };
            if kind != run_kind && !run.is_empty() {
                self.flush_span(&mut rendered, &run, run_kind, selected);
                run.clear();
            }
            run_kind = kind;
            run.push(c);
        }
        self.flush_span(&mut rendered, &run, run_kind, selected);
        if truncated {
            rendered.push('…');
        }

        let left = format!("{}{} {}", self.arrow(selected), icon, rendered);
        let left_w = prefix_w + width(name) + usize::from(truncated);
        let time = format!("{}, {:.1}", relative_time(self.now, e.mtime), m.score);
        let mut meta = String::new();
        let mut meta_w = width(&time);
        if let Some((glyphs, glyph_w)) = self.git_marker(&e.name) {
            meta.push_str(&glyphs);
            if selected {
                // Marker colors reset the foreground; restore the row's.
                meta.push_str(st.sel_fg());
            }
            meta.push(' ');
            meta_w += glyph_w + 1;
        }
        meta.push_str(&if selected { time } else { st.dim(&time) });
        compose(&left, left_w, Some((&meta, meta_w)), w, &bg)
    }

    fn flush_span(&self, out: &mut String, text: &str, kind: Span, selected: bool) {
        match kind {
            Span::Plain => out.push_str(text),
            Span::Date => out.push_str(&self.style.dim(text)),
            Span::Match => {
                out.push_str(&self.style.highlight(text));
                if selected {
                    out.push_str(self.style.sel_fg());
                }
            }
        }
    }

    fn create_line(&self, selected: bool, w: usize) -> String {
        let label = format!("📂 Create new: {}-{}", self.today, self.input.text());
        let left = format!("{}{}", self.arrow(selected), label);
        let bg = if selected { self.style.selected() } else { "" };
        compose(&left, 2 + width(&label), None, w, bg)
    }

    fn render_dialog(&self, kind: &Kind, d: &Dialog, w: usize) -> Screen {
        let st = self.style;
        let (icon, title, footer_hint) = match kind {
            Kind::Rename { .. } => (
                "📝",
                "Rename directory".to_string(),
                "Enter: Confirm  Esc: Cancel",
            ),
            Kind::Graduate { .. } => (
                "🚀",
                "Graduate try to project".to_string(),
                "Enter: Confirm  Esc: Cancel",
            ),
            Kind::NewName => ("📂", "New try".to_string(), "Enter: Create  Esc: Cancel"),
            Kind::ConfirmDelete { .. } => {
                let n = self.marked.len();
                (
                    "🗑️",
                    format!(
                        "Delete {n} {}?",
                        if n == 1 { "directory" } else { "directories" }
                    ),
                    "Enter: Confirm  Esc: Cancel",
                )
            }
        };

        let header = vec![
            centered(
                &format!("{icon}{}", st.accent(&format!("  {title}"))),
                width(icon) + 2 + width(&title),
                w,
            ),
            self.sep(w),
        ];

        let mut body = Vec::new();
        let prompt = |label: &str| {
            let text = format!("{}{}", st.dim(label), d.input.render(st));
            centered(&text, width(label) + d.input.visible_width(), w)
        };
        match kind {
            Kind::Rename { name } => {
                body.push(format!("📁 {name}"));
                body.extend([String::new(), String::new(), prompt("New name: ")]);
            }
            Kind::Graduate { name, .. } => {
                let hint = format!("Destination (default: {})", self.projects_dir.display());
                body.push(format!("📁 {name}"));
                body.push(String::new());
                body.push(centered(&st.dim(&hint), width(&hint), w));
                body.push(prompt("Move to: "));
                body.push(String::new());
                let note = "A symlink will be left in the tries directory";
                body.push(centered(&st.dim(note), width(note), w));
            }
            Kind::NewName => {
                body.push(format!("Enter new try name  ({}-…)", self.today));
                body.extend([String::new(), prompt("Name: ")]);
            }
            Kind::ConfirmDelete { warnings } => {
                for name in &self.marked {
                    let text = format!("🗑️ {name}");
                    let tw = width(&text);
                    body.push(pad_bg(
                        &format!("{}{}", st.danger_bg(), text),
                        tw,
                        w,
                        st.danger_bg(),
                    ));
                }
                for warning in warnings {
                    let text = format!("⚠ {warning}");
                    body.push(centered(&st.warn(&text), width(&text), w));
                }
                let note = if self.trash {
                    format!("Moved to {}/ — restore with: try undo", crate::trash::DIR)
                } else {
                    "Permanently deleted; this cannot be undone".to_string()
                };
                body.push(String::new());
                body.push(centered(&st.dim(&note), width(&note), w));
                body.extend([String::new(), prompt("Type YES to confirm: ")]);
            }
        }
        if let Some(err) = &d.error {
            body.push(String::new());
            body.push(centered(&st.bold(err), width(err), w));
        }

        let footer = vec![
            self.sep(w),
            centered(&st.dim(footer_hint), width(footer_hint), w),
        ];
        Screen {
            header,
            body,
            footer,
        }
    }
}

/// Styled git glyphs and their width: `●` uncommitted, `↑n` unpushed,
/// `↓n` behind upstream, `✓` clean and in sync.
fn marker(st: &Style, g: &GitStatus) -> (String, usize) {
    let mut out = String::new();
    let mut w = 0;
    let mut put = |styled: String, text: &str| {
        out.push_str(&styled);
        w += width(text);
    };
    if g.dirty {
        put(st.warn("●"), "●");
    }
    if g.ahead > 0 {
        let t = format!("↑{}", g.ahead);
        put(st.warn(&t), &t);
    }
    if g.behind > 0 {
        let t = format!("↓{}", g.behind);
        put(st.dim(&t), &t);
    }
    if !g.dirty && g.ahead == 0 && g.behind == 0 {
        put(st.good("✓"), "✓");
    }
    (out, w)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Span {
    Plain,
    Date,
    Match,
}

/// `~`-expand and absolutize a destination typed by the user.
fn expand_dest(text: &str) -> Result<PathBuf, String> {
    if text.is_empty() {
        return Err("Destination cannot be empty".into());
    }
    let expanded = if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        Path::new(&home).join(text.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(text)
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        std::env::current_dir()
            .map(|c| c.join(expanded))
            .map_err(|e| e.to_string())
    }
}

/// `just now`, `5m ago`, `3h ago`, `2d ago`, `4w ago`.
pub fn relative_time(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map(|d| d.as_secs()).unwrap_or(0);
    let (m, h, d) = (secs / 60, secs / 3600, secs / 86_400);
    match secs {
        0..=59 => "just now".into(),
        _ if m < 60 => format!("{m}m ago"),
        _ if h < 24 => format!("{h}h ago"),
        _ if d < 7 => format!("{d}d ago"),
        _ => format!("{}w ago", d / 7),
    }
}

/// Centre already-styled `text` (visible width `text_w`) on a `w`-column line.
fn centered(text: &str, text_w: usize, w: usize) -> String {
    let lead = w.saturating_sub(1).saturating_sub(text_w) / 2;
    format!("{}{}", " ".repeat(lead), text)
}

/// Extend a background to the right edge (`w - 1` columns).
fn pad_bg(styled: &str, visible_w: usize, w: usize, bg: &str) -> String {
    let fill = w.saturating_sub(1).saturating_sub(visible_w);
    if bg.is_empty() {
        styled.to_string()
    } else {
        format!("{styled}{}{RESET}", " ".repeat(fill))
    }
}

/// Lay out one row: `left` flush left, optional `right` flush to the edge when
/// there is at least a two-column gap, background filled to the edge.
fn compose(left: &str, left_w: usize, right: Option<(&str, usize)>, w: usize, bg: &str) -> String {
    let max = w.saturating_sub(1);
    let mut line = format!("{bg}{left}");
    let mut used = left_w;
    if let Some((meta, meta_w)) = right {
        if used + 2 + meta_w <= max {
            line.push_str(&" ".repeat(max - used - meta_w));
            line.push_str(meta);
            used = max;
        }
    }
    if !bg.is_empty() {
        line.push_str(&" ".repeat(max.saturating_sub(used)));
        line.push_str(RESET);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(name: &str, age_secs: u64, now: SystemTime) -> Entry {
        let mtime = now - Duration::from_secs(age_secs);
        Entry {
            name: name.into(),
            path: PathBuf::from("/tries").join(name),
            is_symlink: false,
            mtime,
            base_score: entries::base_score(name, mtime, now),
        }
    }

    fn selector(names: &[(&str, u64)], query: &str) -> Selector {
        let now = SystemTime::now();
        let entries = names.iter().map(|(n, a)| entry(n, *a, now)).collect();
        let mut s = Selector::with_entries(
            PathBuf::from("/tries"),
            entries,
            query,
            PathBuf::from("/projects"),
            Style { color: false },
            "2025-12-01".into(),
            now,
        );
        s.set_height(24);
        s
    }

    fn press(s: &mut Selector, keys: &[Key]) -> Option<Outcome> {
        keys.iter().find_map(|k| s.handle_key(*k))
    }

    fn type_str(s: &str) -> Vec<Key> {
        s.chars().map(Key::Char).collect()
    }

    fn text(screen: &Screen) -> String {
        [&screen.header, &screen.body, &screen.footer]
            .iter()
            .flat_map(|v| v.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn most_recent_is_first_and_enter_selects() {
        let mut s = selector(&[("2025-01-01-old", 90_000), ("2025-01-02-new", 10)], "");
        assert_eq!(
            press(&mut s, &[Key::Enter]),
            Some(Outcome::Cd("/tries/2025-01-02-new".into()))
        );
    }

    #[test]
    fn navigation_clamps_and_includes_create_row() {
        let mut s = selector(&[("aaa", 1), ("aab", 2)], "aa");
        // 2 matches + create row = 3 items
        press(&mut s, &[Key::Down, Key::Down, Key::Down, Key::Down]);
        assert_eq!(
            press(&mut s, &[Key::Enter]),
            Some(Outcome::Mkdir("/tries/2025-12-01-aa".into()))
        );

        let mut s = selector(&[("aaa", 1), ("aab", 2)], "aa");
        press(&mut s, &[Key::Up, Key::Up]);
        assert!(matches!(press(&mut s, &[Key::Enter]), Some(Outcome::Cd(_))));
    }

    #[test]
    fn vim_and_emacs_navigation_are_symmetric() {
        for (down, up) in [
            (Key::Ctrl('j'), Key::Ctrl('k')),
            (Key::Ctrl('n'), Key::Ctrl('p')),
        ] {
            let mut s = selector(&[("a", 1), ("b", 100_000)], "");
            let first = press(&mut s, &[Key::Enter]);
            let mut s = selector(&[("a", 1), ("b", 100_000)], "");
            let round = press(&mut s, &[down, up, Key::Enter]);
            assert_eq!(first, round);
            let mut s = selector(&[("a", 1), ("b", 100_000)], "");
            assert_ne!(first, press(&mut s, &[down, Key::Enter]));
        }
    }

    #[test]
    fn typing_filters_and_resets_cursor() {
        let mut s = selector(&[("alpha", 1), ("beta", 2)], "");
        press(&mut s, &[Key::Down]);
        press(&mut s, &type_str("alp"));
        assert_eq!(
            press(&mut s, &[Key::Enter]),
            Some(Outcome::Cd("/tries/alpha".into()))
        );
    }

    #[test]
    fn create_new_sanitizes_and_empty_query_prompts() {
        let mut s = selector(&[], "my idea/x");
        assert_eq!(
            press(&mut s, &[Key::Enter]),
            Some(Outcome::Mkdir("/tries/2025-12-01-my-idea-x".into()))
        );

        let mut s = selector(&[("a", 1)], "");
        assert_eq!(press(&mut s, &[Key::Ctrl('t')]), None);
        let out = press(
            &mut s,
            &[Key::Char('n'), Key::Char(' '), Key::Char('x'), Key::Enter],
        );
        assert_eq!(out, Some(Outcome::Mkdir("/tries/2025-12-01-n-x".into())));

        let mut s = selector(&[("a", 1)], "");
        press(&mut s, &[Key::Ctrl('t')]);
        assert_eq!(press(&mut s, &[Key::Enter]), None); // empty name closes the prompt
        assert_eq!(press(&mut s, &[Key::Esc]), Some(Outcome::Cancel));
    }

    #[test]
    fn escape_cancels_but_first_leaves_delete_mode() {
        let mut s = selector(&[("a", 1)], "");
        press(&mut s, &[Key::Ctrl('d')]);
        assert_eq!(s.handle_key(Key::Esc), None);
        assert_eq!(s.handle_key(Key::Esc), Some(Outcome::Cancel));
        assert_eq!(
            selector(&[("a", 1)], "").handle_key(Key::Ctrl('c')),
            Some(Outcome::Cancel)
        );
    }

    #[test]
    fn rename_validation() {
        let mut s = selector(&[("one", 1), ("two", 2)], "");
        press(&mut s, &[Key::Ctrl('r'), Key::Ctrl('u')]);
        // empty
        assert_eq!(press(&mut s, &[Key::Enter]), None);
        assert!(text(&s.render(80, 24)).contains("Name cannot be empty"));
        // slash
        press(&mut s, &type_str("a/b"));
        press(&mut s, &[Key::Enter]);
        assert!(text(&s.render(80, 24)).contains("Name cannot contain /"));
        // success, whitespace dashified
        press(&mut s, &[Key::Ctrl('u')]);
        press(&mut s, &type_str("new name"));
        assert_eq!(
            press(&mut s, &[Key::Enter]),
            Some(Outcome::Rename {
                old: "one".into(),
                new: "new-name".into()
            })
        );
        // unchanged name closes quietly
        let mut s = selector(&[("one", 1)], "");
        assert_eq!(press(&mut s, &[Key::Ctrl('r'), Key::Enter]), None);
        assert_eq!(press(&mut s, &[Key::Esc]), Some(Outcome::Cancel));
    }

    #[test]
    fn rename_rejects_existing_directory() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("one")).unwrap();
        fs::create_dir(t.path().join("two")).unwrap();
        let mut s = Selector::open(t.path(), "", t.path().into(), Style { color: false }).unwrap();
        s.set_height(24);
        press(&mut s, &[Key::Ctrl('r'), Key::Ctrl('u')]);
        press(&mut s, &type_str("two"));
        press(&mut s, &[Key::Enter]);
        assert!(text(&s.render(80, 24)).contains("Directory exists: two"));
    }

    #[test]
    fn delete_requires_typing_yes_and_stays_inside_base() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("tries");
        fs::create_dir_all(base.join("keep")).unwrap();
        fs::create_dir_all(base.join("doomed")).unwrap();
        let mut s =
            Selector::open(&base, "doomed", t.path().into(), Style { color: false }).unwrap();
        s.set_height(24);
        press(&mut s, &[Key::Ctrl('d'), Key::Enter]);
        assert!(s.awaiting_delete_confirmation());
        // wrong confirmation cancels
        assert_eq!(press(&mut s, &[Key::Char('y'), Key::Enter]), None);
        assert!(!s.awaiting_delete_confirmation());
        assert!(text(&s.render(80, 24)).contains("Delete cancelled"));

        press(&mut s, &[Key::Ctrl('d'), Key::Enter]);
        let out = press(
            &mut s,
            &type_str("YES")
                .into_iter()
                .chain([Key::Enter])
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            out,
            Some(Outcome::Delete {
                base: fs::canonicalize(&base).unwrap(),
                names: vec!["doomed".into()]
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_refuses_symlink_escaping_the_tries_root() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("tries");
        let outside = t.path().join("precious");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("link")).unwrap();

        let mut s = Selector::open(&base, "", t.path().into(), Style { color: false }).unwrap();
        s.set_height(24);
        let keys: Vec<Key> = [Key::Ctrl('d'), Key::Enter]
            .into_iter()
            .chain(type_str("YES"))
            .chain([Key::Enter])
            .collect();
        assert_eq!(press(&mut s, &keys), None);
        let screen = text(&s.render(80, 24));
        assert!(screen.contains("Error: Safety check failed"), "{screen}");
    }

    #[test]
    fn graduate_validates_destination() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("tries");
        fs::create_dir_all(base.join("2025-01-01-idea")).unwrap();
        fs::create_dir_all(t.path().join("taken")).unwrap();
        let mut s = Selector::open(&base, "", t.path().into(), Style { color: false }).unwrap();
        s.set_height(24);

        // Default destination is projects_dir/<name without date>.
        let out = press(&mut s, &[Key::Ctrl('g'), Key::Enter]);
        assert_eq!(
            out,
            Some(Outcome::Graduate {
                source: base.join("2025-01-01-idea"),
                dest: t.path().join("idea"),
                name: "2025-01-01-idea".into()
            })
        );

        let mut s = Selector::open(&base, "", t.path().into(), Style { color: false }).unwrap();
        s.set_height(24);
        press(&mut s, &[Key::Ctrl('g'), Key::Ctrl('u')]);
        press(&mut s, &type_str(&format!("{}/taken", t.path().display())));
        press(&mut s, &[Key::Enter]);
        assert!(text(&s.render(80, 24)).contains("Destination already exists"));
        press(&mut s, &[Key::Ctrl('u')]);
        press(
            &mut s,
            &type_str(&format!("{}/nope/deeper", t.path().display())),
        );
        press(&mut s, &[Key::Enter]);
        assert!(text(&s.render(80, 24)).contains("Parent directory does not exist"));
    }

    #[test]
    fn relative_time_buckets() {
        let now = SystemTime::now();
        let ago = |s: u64| relative_time(now, now - Duration::from_secs(s));
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(300), "5m ago");
        assert_eq!(ago(7_200), "2h ago");
        assert_eq!(ago(3 * 86_400), "3d ago");
        assert_eq!(ago(15 * 86_400), "2w ago");
        assert_eq!(
            relative_time(now, now + Duration::from_secs(100)),
            "just now"
        );
    }

    #[test]
    fn render_layout_and_truncation() {
        let mut s = selector(
            &[
                ("2025-11-01-alpha", 5),
                (
                    "2025-11-30-this-is-a-very-long-directory-name-that-overflows-the-line",
                    9,
                ),
            ],
            "",
        );
        let screen = s.render(60, 24);
        let rows: Vec<&String> = screen.body.iter().collect();
        assert!(rows[0].starts_with("→ 📁 2025-11-01-alpha"));
        assert!(
            rows[0].ends_with("just now, 5.0") || rows[0].contains("just now, "),
            "{}",
            rows[0]
        );
        assert!(rows[1].contains('…'));
        assert!(
            !rows[1].contains("ago") && !rows[1].contains("just now"),
            "metadata must hide: {}",
            rows[1]
        );
        for row in screen
            .header
            .iter()
            .chain(&screen.body)
            .chain(&screen.footer)
        {
            assert!(width(&strip(row)) <= 59, "row too wide: {row:?}");
        }
    }

    #[test]
    fn create_row_follows_matches_with_spacer() {
        let mut s = selector(&[("alpha", 1)], "al");
        let screen = s.render(80, 24);
        assert_eq!(screen.body.len(), 3);
        assert!(screen.body[1].is_empty());
        assert!(screen.body[2].contains("Create new: 2025-12-01-al"));
    }

    #[test]
    fn scrolls_to_keep_cursor_visible() {
        let names: Vec<String> = (0..30).map(|i| format!("dir{i:02}")).collect();
        let items: Vec<(&str, u64)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i as u64 * 10))
            .collect();
        let mut s = selector(&items, "dir");
        let h = 12;
        for _ in 0..8 {
            s.handle_key(Key::Down);
        }
        let screen = s.render(80, h);
        assert!(screen.header.len() + screen.body.len() + screen.footer.len() <= h);
        assert!(
            screen.body.iter().any(|l| l.starts_with("→")),
            "cursor row must be visible"
        );
    }

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for n in it.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
