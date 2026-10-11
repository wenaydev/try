//! A trash for deleted tries, kept *inside* the tries root.
//!
//! Layout: `<tries>/.trash/<batch-id>/<name>`. It is on the same filesystem, so
//! moving in and out is an atomic rename, and the leading dot keeps it out of
//! the selector list. A batch is everything deleted in one confirmation; the
//! id is the deletion time in nanoseconds since the epoch, zero-padded.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DIR: &str = ".trash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub id: String,
    pub path: PathBuf,
    pub names: Vec<String>,
    /// Nanoseconds since the epoch, parsed from the id.
    pub deleted_nanos: u128,
}

impl Batch {
    pub fn age(&self, now: SystemTime) -> Duration {
        let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        Duration::from_nanos(now.saturating_sub(self.deleted_nanos).min(u64::MAX as u128) as u64)
    }
}

pub fn new_batch_id() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{n:024}")
}

pub fn dir(base: &Path) -> PathBuf {
    base.join(DIR)
}

/// All batches, oldest first. A missing trash is an empty trash.
pub fn batches(base: &Path) -> io::Result<Vec<Batch>> {
    let root = dir(base);
    let rd = match fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for item in rd.flatten() {
        let Ok(id) = item.file_name().into_string() else {
            continue;
        };
        let Ok(deleted_nanos) = id.parse::<u128>() else {
            continue;
        };
        if !item.path().is_dir() {
            continue;
        }
        let mut names: Vec<String> = fs::read_dir(item.path())?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        out.push(Batch {
            id,
            path: item.path(),
            names,
            deleted_nanos,
        });
    }
    out.sort_by_key(|b| b.deleted_nanos);
    Ok(out)
}

/// Decide where each trashed name goes when restored into `base`: its own
/// name if free, otherwise `<name>-restored`, `-restored-2`, ... Names chosen
/// for earlier items are reserved so two items never collide.
pub fn plan_restore(base: &Path, names: &[String]) -> Vec<(String, String)> {
    let mut used: Vec<String> = Vec::new();
    let free = |n: &str, used: &[String]| {
        fs::symlink_metadata(base.join(n)).is_err() && !used.iter().any(|u| u == n)
    };
    names
        .iter()
        .map(|name| {
            let target = if free(name, &used) {
                name.clone()
            } else {
                let first = format!("{name}-restored");
                if free(&first, &used) {
                    first
                } else {
                    (2u64..)
                        .map(|i| format!("{first}-{i}"))
                        .find(|n| free(n, &used))
                        .expect("u64 suffix space exhausted")
                }
            };
            used.push(target.clone());
            (name.clone(), target)
        })
        .collect()
}

/// Batches old enough to purge (all of them when `older_than` is `None`).
pub fn expired(
    base: &Path,
    older_than: Option<Duration>,
    now: SystemTime,
) -> io::Result<Vec<Batch>> {
    Ok(batches(base)?
        .into_iter()
        .filter(|b| older_than.is_none_or(|min| b.age(now) >= min))
        .collect())
}

/// Permanently delete those batches. Returns `(batches_removed, items_removed)`.
pub fn empty(
    base: &Path,
    older_than: Option<Duration>,
    now: SystemTime,
) -> io::Result<(usize, usize)> {
    let (mut nb, mut ni) = (0, 0);
    for b in expired(base, older_than, now)? {
        fs::remove_dir_all(&b.path)?;
        nb += 1;
        ni += b.names.len();
    }
    // Leave no empty `.trash` husk behind.
    let _ = fs::remove_dir(dir(base));
    Ok((nb, ni))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(base: &Path, id: u128, names: &[&str]) {
        for n in names {
            fs::create_dir_all(dir(base).join(format!("{id:024}")).join(n)).unwrap();
        }
    }

    #[test]
    fn batches_sorted_and_listing_ignores_junk() {
        let t = tempfile::tempdir().unwrap();
        assert!(batches(t.path()).unwrap().is_empty());
        put(t.path(), 200, &["b", "a"]);
        put(t.path(), 100, &["x"]);
        fs::write(dir(t.path()).join("stray-file"), "").unwrap();
        fs::create_dir_all(dir(t.path()).join("not-a-number")).unwrap();
        let b = batches(t.path()).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].names, ["x"]);
        assert_eq!(b[1].names, ["a", "b"]);
        assert!(b[0].deleted_nanos < b[1].deleted_nanos);
    }

    #[test]
    fn restore_names_never_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let plan = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
            plan_restore(t.path(), &names)
                .into_iter()
                .map(|(_, to)| to)
                .collect::<Vec<_>>()
        };
        assert_eq!(plan(&["x"]), ["x"]);
        fs::create_dir(t.path().join("x")).unwrap();
        assert_eq!(plan(&["x"]), ["x-restored"]);
        fs::create_dir(t.path().join("x-restored")).unwrap();
        assert_eq!(plan(&["x"]), ["x-restored-2"]);
        // An item literally named like another item's fallback must not collide.
        fs::remove_dir(t.path().join("x-restored")).unwrap();
        assert_eq!(
            plan(&["x", "x-restored"]),
            ["x-restored", "x-restored-restored"]
        );
    }

    #[test]
    fn empty_honours_age_and_cleans_up() {
        let t = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let nanos = now.duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let day = Duration::from_secs(86_400).as_nanos();
        put(t.path(), nanos - 40 * day, &["old1", "old2"]);
        put(t.path(), nanos - day, &["recent"]);

        let (nb, ni) = empty(t.path(), Some(Duration::from_secs(30 * 86_400)), now).unwrap();
        assert_eq!((nb, ni), (1, 2));
        assert_eq!(batches(t.path()).unwrap().len(), 1);

        let (nb, ni) = empty(t.path(), None, now).unwrap();
        assert_eq!((nb, ni), (1, 1));
        assert!(!dir(t.path()).exists());
    }

    #[test]
    fn ids_sort_chronologically_as_strings() {
        let a = new_batch_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = new_batch_id();
        assert!(a < b && a.len() == 24);
    }
}
