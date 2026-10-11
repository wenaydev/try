//! `try doctor`: check that the installation can actually work.

use crate::config::{Config, DeleteMode, Settings};
use crate::init::{Shell, MARKER};
use crate::trash;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub label: &'static str,
    pub detail: String,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        write!(f, "{tag}  {:<9} {}", self.label, self.detail)
    }
}

fn check(level: Level, label: &'static str, detail: impl Into<String>) -> Check {
    Check {
        level,
        label,
        detail: detail.into(),
    }
}

/// Everything the checks look at, gathered by the caller.
pub struct Inputs<'a> {
    pub settings: &'a Settings,
    pub config_file: Option<&'a Path>,
    pub config: &'a Result<Config, String>,
    pub home: Option<&'a Path>,
    pub exe: Option<&'a Path>,
    pub shell: Option<Shell>,
}

/// Rc files where a `try` wrapper may live.
fn rc_candidates(home: &Path) -> Vec<PathBuf> {
    [
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".zshrc",
        ".zprofile",
        ".config/fish/config.fish",
    ]
    .iter()
    .map(|f| home.join(f))
    .collect()
}

fn wrapper_installed(home: &Path) -> Option<PathBuf> {
    rc_candidates(home).into_iter().find(|rc| {
        fs::read_to_string(rc).is_ok_and(|t| {
            t.contains(MARKER) || t.contains("try init") || t.contains("try.rb init")
        })
    })
}

/// Can we create (and remove) a file in `dir`?
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".try-doctor-{}", std::process::id()));
    let ok = fs::File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

pub fn run(i: &Inputs) -> Vec<Check> {
    let mut out = Vec::new();
    let s = i.settings;

    out.push(check(
        Level::Ok,
        "binary",
        format!(
            "try {} at {}",
            env!("CARGO_PKG_VERSION"),
            i.exe.map_or("?".into(), |p| p.display().to_string())
        ),
    ));

    // Config file
    let cfg_loc = i
        .config_file
        .map_or("(no HOME)".into(), |p| p.display().to_string());
    out.push(match i.config {
        Err(e) => check(Level::Fail, "config", e.clone()),
        Ok(cfg) if !cfg.unknown.is_empty() => check(
            Level::Warn,
            "config",
            format!(
                "unknown key(s) {} in {cfg_loc} (known: {})",
                cfg.unknown.join(", "),
                crate::config::KNOWN_KEYS.join(", ")
            ),
        ),
        Ok(_) if i.config_file.is_some_and(Path::exists) => {
            check(Level::Ok, "config", format!("{cfg_loc} (valid)"))
        }
        Ok(_) => check(
            Level::Ok,
            "config",
            format!("none at {cfg_loc}; using defaults (`try config` shows a template)"),
        ),
    });

    // Tries directory
    let t = &s.tries;
    out.push(if t.is_dir() {
        if writable(t) {
            let n = crate::entries::load(t).map_or(0, |e| e.len());
            check(
                Level::Ok,
                "tries",
                format!("{} ({n} dirs, from {})", t.display(), s.tries_source),
            )
        } else {
            check(
                Level::Fail,
                "tries",
                format!("{} is not writable", t.display()),
            )
        }
    } else if t.exists() {
        check(
            Level::Fail,
            "tries",
            format!("{} exists but is not a directory", t.display()),
        )
    } else {
        let parent_ok = t
            .ancestors()
            .skip(1)
            .find(|a| a.is_dir())
            .is_some_and(writable);
        if parent_ok {
            check(
                Level::Warn,
                "tries",
                format!(
                    "{} does not exist yet; it is created on first use",
                    t.display()
                ),
            )
        } else {
            check(
                Level::Fail,
                "tries",
                format!("{} cannot be created (no writable parent)", t.display()),
            )
        }
    });

    // Shell wrapper
    out.push(match (i.home, i.shell) {
        (Some(home), shell) => match wrapper_installed(home) {
            Some(rc) => check(Level::Ok, "wrapper", format!("found in {}", rc.display())),
            None => check(
                Level::Warn,
                "wrapper",
                format!(
                    "not found in your shell rc files; run `try install` ({})",
                    shell.map_or("shell not detected".to_string(), |s| format!("{s:?}")
                        .to_lowercase())
                ),
            ),
        },
        (None, _) => check(
            Level::Warn,
            "wrapper",
            "HOME is not set; cannot look for the wrapper",
        ),
    });

    // The wrapper sends the UI to /dev/tty.
    #[cfg(unix)]
    out.push(
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            Ok(_) => check(Level::Ok, "tty", "/dev/tty is available"),
            Err(e) => check(
                Level::Warn,
                "tty",
                format!("cannot open /dev/tty ({e}); the interactive UI needs a terminal"),
            ),
        },
    );

    // git
    out.push(match Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => check(
            Level::Ok,
            "git",
            format!(
                "{}; list markers {}",
                String::from_utf8_lossy(&o.stdout).trim(),
                if s.git_status { "on" } else { "off" }
            ),
        ),
        _ => check(
            Level::Warn,
            "git",
            "not found on PATH; clone and worktree will fail",
        ),
    });

    // Graduate destination
    out.push(if s.projects.is_dir() {
        check(
            Level::Ok,
            "projects",
            format!("{} (from {})", s.projects.display(), s.projects_source),
        )
    } else {
        check(
            Level::Warn,
            "projects",
            format!(
                "{} does not exist; Ctrl-G graduate needs it",
                s.projects.display()
            ),
        )
    });

    // Trash
    out.push(match (s.delete, trash::batches(t)) {
        (DeleteMode::Remove, _) => check(
            Level::Warn,
            "delete",
            format!(
                "mode is \"remove\" (from {}): Ctrl-D permanently deletes",
                s.delete_source
            ),
        ),
        (DeleteMode::Trash, Ok(b)) => {
            let items: usize = b.iter().map(|b| b.names.len()).sum();
            let oldest = b
                .first()
                .map(|b| b.age(SystemTime::now()).as_secs() / 86_400);
            check(
                Level::Ok,
                "delete",
                match oldest {
                    Some(d) => format!(
                        "trash mode; {items} item(s) in {} batch(es), oldest {d}d old",
                        b.len()
                    ),
                    None => "trash mode; trash is empty".to_string(),
                },
            )
        }
        (DeleteMode::Trash, Err(e)) => {
            check(Level::Fail, "delete", format!("cannot read trash: {e}"))
        }
    });

    out.push(check(
        Level::Ok,
        "colors",
        if s.color {
            "on"
        } else {
            "off (NO_COLOR, --no-colors or config)"
        },
    ));
    out
}

/// True when nothing failed.
pub fn healthy(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.level != Level::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Source;

    fn settings(tries: &Path, delete: DeleteMode) -> Settings {
        Settings {
            tries: tries.to_path_buf(),
            tries_source: Source::Flag,
            projects: tries.parent().unwrap().to_path_buf(),
            projects_source: Source::Default,
            delete,
            delete_source: Source::Default,
            color: true,
            git_status: true,
            git_status_source: Source::Default,
        }
    }

    fn find<'a>(checks: &'a [Check], label: &str) -> &'a Check {
        checks.iter().find(|c| c.label == label).unwrap()
    }

    #[test]
    fn healthy_setup() {
        let root = tempfile::tempdir().unwrap();
        let tries = root.path().join("tries");
        fs::create_dir_all(tries.join("a")).unwrap();
        fs::write(
            root.path().join(".zshrc"),
            format!("{MARKER}\ntry() {{ :; }}\n"),
        )
        .unwrap();
        let cfg = Ok(Config::default());
        let s = settings(&tries, DeleteMode::Trash);
        let checks = run(&Inputs {
            settings: &s,
            config_file: None,
            config: &cfg,
            home: Some(root.path()),
            exe: Some(Path::new("/x/try")),
            shell: Some(Shell::Zsh),
        });
        assert!(healthy(&checks));
        assert_eq!(find(&checks, "tries").level, Level::Ok);
        assert!(find(&checks, "tries").detail.contains("1 dirs"));
        assert_eq!(find(&checks, "wrapper").level, Level::Ok);
        assert!(find(&checks, "delete").detail.contains("trash is empty"));
    }

    #[test]
    fn problems_are_classified() {
        let root = tempfile::tempdir().unwrap();
        // tries path is a file; config broken; no wrapper
        let file = root.path().join("tries");
        fs::write(&file, "").unwrap();
        let cfg = Err("bad.toml: oops".to_string());
        let s = settings(&file, DeleteMode::Remove);
        let checks = run(&Inputs {
            settings: &s,
            config_file: None,
            config: &cfg,
            home: Some(root.path()),
            exe: None,
            shell: None,
        });
        assert!(!healthy(&checks));
        assert_eq!(find(&checks, "tries").level, Level::Fail);
        assert_eq!(find(&checks, "config").level, Level::Fail);
        assert_eq!(find(&checks, "wrapper").level, Level::Warn);
        assert!(find(&checks, "delete").detail.contains("permanently"));
    }

    #[test]
    fn missing_tries_dir_is_only_a_warning_when_creatable() {
        let root = tempfile::tempdir().unwrap();
        let s = settings(&root.path().join("deep/er/tries"), DeleteMode::Trash);
        let cfg = Ok(Config {
            unknown: vec!["pth".into()],
            ..Default::default()
        });
        let checks = run(&Inputs {
            settings: &s,
            config_file: None,
            config: &cfg,
            home: None,
            exe: None,
            shell: None,
        });
        assert_eq!(find(&checks, "tries").level, Level::Warn);
        assert_eq!(find(&checks, "config").level, Level::Warn);
        assert!(find(&checks, "config").detail.contains("pth"));
        assert!(healthy(&checks));
    }

    #[test]
    fn trash_contents_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let tries = root.path().join("tries");
        fs::create_dir_all(
            trash::dir(&tries)
                .join(format!("{:024}", 1u128))
                .join("gone"),
        )
        .unwrap();
        let cfg = Ok(Config::default());
        let s = settings(&tries, DeleteMode::Trash);
        let checks = run(&Inputs {
            settings: &s,
            config_file: None,
            config: &cfg,
            home: None,
            exe: None,
            shell: None,
        });
        assert!(find(&checks, "delete")
            .detail
            .contains("1 item(s) in 1 batch(es)"));
    }
}
