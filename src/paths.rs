//! Home directory and path normalisation shared by the CLI and config.

use std::env;
use std::path::{Component, Path, PathBuf};

pub fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` and make the path absolute (without resolving symlinks).
pub fn absolutize(p: &str) -> PathBuf {
    let expanded = match p {
        "~" => home().unwrap_or_else(|| p.into()),
        _ => match p.strip_prefix("~/") {
            Some(rest) => home().map(|h| h.join(rest)).unwrap_or_else(|| p.into()),
            None => PathBuf::from(p),
        },
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        env::current_dir()
            .map(|c| c.join(&expanded))
            .unwrap_or(expanded)
    };
    normalize(&absolute)
}

/// Drop `.` and resolve `..` lexically (symlinks are left alone).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolutize_normalizes_dots() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(absolutize("/x/y/."), PathBuf::from("/x/y"));
        assert_eq!(absolutize("/x/y/.."), PathBuf::from("/x"));
        assert_eq!(absolutize("."), env::current_dir().unwrap());
    }
}
