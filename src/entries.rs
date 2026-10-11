//! Loading the directories under the tries root.

use crate::naming;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// Real path (symlinks, e.g. graduated tries, are resolved).
    pub path: PathBuf,
    pub is_symlink: bool,
    pub mtime: SystemTime,
    /// Recency + date-prefix bonus; the starting score before fuzzy matching.
    pub base_score: f64,
}

/// `3 / sqrt(hours_since_touch + 1)`, plus 2 for `YYYY-MM-DD-` names.
pub fn base_score(name: &str, mtime: SystemTime, now: SystemTime) -> f64 {
    let hours = now
        .duration_since(mtime)
        .map(|d| d.as_secs_f64() / 3600.0)
        .unwrap_or(0.0);
    let mut score = 3.0 / (hours + 1.0).sqrt();
    if naming::has_date_prefix(name) {
        score += 2.0;
    }
    score
}

/// List visible subdirectories of `base`, sorted by name so ordering is
/// deterministic whatever order the filesystem returns.
pub fn load(base: &Path) -> io::Result<Vec<Entry>> {
    let now = SystemTime::now();
    let mut out = Vec::new();
    for item in fs::read_dir(base)? {
        let Ok(item) = item else { continue };
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = item.path();
        // `metadata` follows symlinks; dangling links and files drop out here.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let is_symlink = item.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let real = if is_symlink {
            match fs::canonicalize(&path) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            path
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(Entry {
            base_score: base_score(&name, mtime, now),
            name,
            path: real,
            is_symlink,
            mtime,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn score_decays_with_age_and_rewards_dates() {
        let now = SystemTime::now();
        let fresh = base_score("x", now, now);
        let day = base_score("x", now - Duration::from_secs(86_400), now);
        assert!((fresh - 3.0).abs() < 1e-9);
        assert!(day < fresh);
        assert!((base_score("2025-01-01-x", now, now) - 5.0).abs() < 1e-9);
        // clock skew: future mtimes must not produce NaN
        assert!(base_score("x", now + Duration::from_secs(10_000), now).is_finite());
    }

    #[test]
    fn loads_only_visible_directories() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("b")).unwrap();
        fs::create_dir(t.path().join("a")).unwrap();
        fs::create_dir(t.path().join(".hidden")).unwrap();
        fs::write(t.path().join("file"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(t.path().join("a"), t.path().join("link")).unwrap();
            std::os::unix::fs::symlink(t.path().join("nope"), t.path().join("dangling")).unwrap();
        }
        let e = load(t.path()).unwrap();
        let names: Vec<_> = e.iter().map(|e| e.name.as_str()).collect();
        #[cfg(unix)]
        assert_eq!(names, ["a", "b", "link"]);
        #[cfg(not(unix))]
        assert_eq!(names, ["a", "b"]);
        #[cfg(unix)]
        assert!(e[2].is_symlink && e[2].path.ends_with("a"));
    }
}
