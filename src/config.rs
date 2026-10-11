//! Settings: `~/.config/try/config.toml`, environment, and flags.
//!
//! Precedence (highest first): command-line flag, environment variable,
//! config file, built-in default.

use crate::paths::{absolutize, home};
use std::fmt;
use std::path::{Path, PathBuf};

pub const KNOWN_KEYS: &[&str] = &["path", "projects", "delete", "colors", "git_status"];

pub const TEMPLATE: &str = "\
# try configuration

# Where experiments live (default: ~/src/tries). TRY_PATH and --path override it.
# path = \"~/src/tries\"

# Where Ctrl-G (graduate) moves a try (default: parent of `path`). TRY_PROJECTS overrides it.
# projects = \"~/src\"

# What Ctrl-D does: \"trash\" moves to <path>/.trash (restore with `try undo`),
# \"remove\" runs rm -rf. TRY_DELETE overrides it.
# delete = \"trash\"

# Set to false to disable colors (NO_COLOR=1 also does).
# colors = true

# Show git markers in the list (● uncommitted, ↑n unpushed, ✓ clean). TRY_GIT_STATUS=0 disables.
# git_status = true
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    Trash,
    Remove,
}

impl DeleteMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "trash" => Some(Self::Trash),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

impl fmt::Display for DeleteMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Trash => "trash",
            Self::Remove => "remove",
        })
    }
}

/// Raw contents of the config file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Config {
    pub path: Option<String>,
    pub projects: Option<String>,
    pub delete: Option<DeleteMode>,
    pub colors: Option<bool>,
    pub git_status: Option<bool>,
    /// Keys we do not know (typos), reported by `try doctor`.
    pub unknown: Vec<String>,
}

/// `$XDG_CONFIG_HOME/try/config.toml`, else `~/.config/try/config.toml`.
pub fn file_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    xdg.or_else(|| home().map(|h| h.join(".config")))
        .map(|d| d.join("try/config.toml"))
}

/// Read the config file. A missing file is fine; an unreadable or invalid one is an error.
pub fn load() -> Result<Config, String> {
    let Some(path) = file_path() else {
        return Ok(Config::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        // First line of toml's message is the useful one.
        e.to_string()
            .lines()
            .next()
            .unwrap_or("invalid TOML")
            .to_string()
    })?;
    let mut cfg = Config::default();
    for (key, value) in &table {
        let string = || {
            value
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("`{key}` must be a non-empty string"))
        };
        match key.as_str() {
            "path" => cfg.path = Some(string()?),
            "projects" => cfg.projects = Some(string()?),
            "delete" => {
                let s = string()?;
                cfg.delete = Some(
                    DeleteMode::parse(&s)
                        .ok_or_else(|| "`delete` must be \"trash\" or \"remove\"".to_string())?,
                );
            }
            "colors" => {
                cfg.colors = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| "`colors` must be true or false".to_string())?,
                )
            }
            "git_status" => {
                cfg.git_status = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| "`git_status` must be true or false".to_string())?,
                )
            }
            other => cfg.unknown.push(other.to_string()),
        }
    }
    Ok(cfg)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env(&'static str),
    Config,
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Flag => f.write_str("--path flag"),
            Source::Env(name) => write!(f, "${name}"),
            Source::Config => f.write_str("config file"),
            Source::Default => f.write_str("default"),
        }
    }
}

/// Effective settings after applying precedence.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub tries: PathBuf,
    pub tries_source: Source,
    pub projects: PathBuf,
    pub projects_source: Source,
    pub delete: DeleteMode,
    pub delete_source: Source,
    pub color: bool,
    pub git_status: bool,
    pub git_status_source: Source,
}

/// `1/true/on/yes` and `0/false/off/no`, case-insensitive.
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

impl Settings {
    /// `env` looks up an environment variable (empty values count as unset).
    pub fn resolve(
        cfg: &Config,
        path_flag: Option<&str>,
        no_color_flag: bool,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Settings, String> {
        let env = |k: &str| env(k).filter(|v| !v.is_empty());

        let (tries, tries_source) = if let Some(p) = path_flag {
            (absolutize(p), Source::Flag)
        } else if let Some(p) = env("TRY_PATH") {
            (absolutize(&p), Source::Env("TRY_PATH"))
        } else if let Some(p) = &cfg.path {
            (absolutize(p), Source::Config)
        } else {
            (
                home().unwrap_or_default().join("src/tries"),
                Source::Default,
            )
        };

        let (projects, projects_source) = if let Some(p) = env("TRY_PROJECTS") {
            (absolutize(&p), Source::Env("TRY_PROJECTS"))
        } else if let Some(p) = &cfg.projects {
            (absolutize(p), Source::Config)
        } else {
            (
                tries.parent().map(Path::to_path_buf).unwrap_or_default(),
                Source::Default,
            )
        };

        let (delete, delete_source) = if let Some(v) = env("TRY_DELETE") {
            let mode = DeleteMode::parse(&v).ok_or_else(|| {
                format!("TRY_DELETE must be \"trash\" or \"remove\", got \"{v}\"")
            })?;
            (mode, Source::Env("TRY_DELETE"))
        } else if let Some(m) = cfg.delete {
            (m, Source::Config)
        } else {
            (DeleteMode::Trash, Source::Default)
        };

        let color = !no_color_flag
            && env("NO_COLOR").is_none()
            && env("NO_COLORS").is_none()
            && cfg.colors != Some(false);

        let (git_status, git_status_source) = if let Some(v) = env("TRY_GIT_STATUS") {
            let b = parse_bool(&v).ok_or_else(|| {
                format!("TRY_GIT_STATUS must be 0/1/true/false/on/off, got \"{v}\"")
            })?;
            (b, Source::Env("TRY_GIT_STATUS"))
        } else if let Some(b) = cfg.git_status {
            (b, Source::Config)
        } else {
            (true, Source::Default)
        };

        Ok(Settings {
            tries,
            tries_source,
            projects,
            projects_source,
            delete,
            delete_source,
            color,
            git_status,
            git_status_source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn parses_all_keys_and_reports_unknown() {
        let c = parse(
            "path = \"/a\"\nprojects = \"/b\"\ndelete = \"remove\"\ncolors = false\npth = \"x\"\n",
        )
        .unwrap();
        assert_eq!(c.path.as_deref(), Some("/a"));
        assert_eq!(c.projects.as_deref(), Some("/b"));
        assert_eq!(c.delete, Some(DeleteMode::Remove));
        assert_eq!(c.colors, Some(false));
        assert_eq!(c.unknown, vec!["pth"]);
        assert_eq!(parse("# only comments\n").unwrap(), Config::default());
    }

    #[test]
    fn rejects_bad_values_with_the_key_name() {
        assert!(parse("path = 3").unwrap_err().contains("`path`"));
        assert!(parse("path = \"\"").unwrap_err().contains("`path`"));
        assert!(parse("delete = \"shred\"")
            .unwrap_err()
            .contains("`delete`"));
        assert!(parse("colors = \"no\"").unwrap_err().contains("`colors`"));
        assert!(parse("path = ").is_err());
    }

    #[test]
    fn precedence_flag_env_config_default() {
        let cfg = Config {
            path: Some("/cfg".into()),
            ..Default::default()
        };
        let s = |flag, env: &[(&str, &str)]| {
            Settings::resolve(&cfg, flag, false, &env_of(env)).unwrap()
        };

        let r = s(Some("/flag"), &[("TRY_PATH", "/env")]);
        assert_eq!(
            (r.tries.as_path(), r.tries_source),
            (Path::new("/flag"), Source::Flag)
        );
        let r = s(None, &[("TRY_PATH", "/env")]);
        assert_eq!(r.tries, PathBuf::from("/env"));
        let r = s(None, &[("TRY_PATH", "")]);
        assert_eq!(
            (r.tries.as_path(), r.tries_source),
            (Path::new("/cfg"), Source::Config)
        );
        let r = Settings::resolve(&Config::default(), None, false, &env_of(&[])).unwrap();
        assert_eq!(r.tries_source, Source::Default);
        assert!(r.tries.ends_with("src/tries"));
    }

    #[test]
    fn projects_default_to_parent_of_tries() {
        let cfg = Config {
            path: Some("/work/tries".into()),
            ..Default::default()
        };
        let r = Settings::resolve(&cfg, None, false, &env_of(&[])).unwrap();
        assert_eq!(r.projects, PathBuf::from("/work"));
        let r = Settings::resolve(&cfg, None, false, &env_of(&[("TRY_PROJECTS", "/p")])).unwrap();
        assert_eq!(r.projects, PathBuf::from("/p"));
    }

    #[test]
    fn delete_mode_defaults_to_trash_and_validates_env() {
        let none = Config::default();
        let r = Settings::resolve(&none, None, false, &env_of(&[])).unwrap();
        assert_eq!(r.delete, DeleteMode::Trash);
        let r =
            Settings::resolve(&none, None, false, &env_of(&[("TRY_DELETE", "remove")])).unwrap();
        assert_eq!(
            (r.delete, r.delete_source),
            (DeleteMode::Remove, Source::Env("TRY_DELETE"))
        );
        let err =
            Settings::resolve(&none, None, false, &env_of(&[("TRY_DELETE", "x")])).unwrap_err();
        assert!(err.contains("TRY_DELETE"));
    }

    #[test]
    fn git_status_setting() {
        let none = Config::default();
        let get = |cfg: &Config, env: &[(&str, &str)]| {
            Settings::resolve(cfg, None, false, &env_of(env))
                .map(|s| (s.git_status, s.git_status_source))
        };
        assert_eq!(get(&none, &[]).unwrap(), (true, Source::Default));
        let off = Config {
            git_status: Some(false),
            ..Default::default()
        };
        assert_eq!(get(&off, &[]).unwrap(), (false, Source::Config));
        assert_eq!(
            get(&off, &[("TRY_GIT_STATUS", "ON")]).unwrap(),
            (true, Source::Env("TRY_GIT_STATUS"))
        );
        assert!(get(&none, &[("TRY_GIT_STATUS", "maybe")])
            .unwrap_err()
            .contains("TRY_GIT_STATUS"));
        assert!(parse("git_status = \"yes\"")
            .unwrap_err()
            .contains("`git_status`"));
    }

    #[test]
    fn color_switches() {
        let off = Config {
            colors: Some(false),
            ..Default::default()
        };
        let on = |cfg: &Config, flag, env: &[(&str, &str)]| {
            Settings::resolve(cfg, None, flag, &env_of(env))
                .unwrap()
                .color
        };
        assert!(on(&Config::default(), false, &[]));
        assert!(!on(&Config::default(), true, &[]));
        assert!(!on(&Config::default(), false, &[("NO_COLOR", "1")]));
        assert!(!on(&off, false, &[]));
    }

    #[test]
    fn template_is_valid_and_inert() {
        assert_eq!(parse(TEMPLATE).unwrap(), Config::default());
    }
}
