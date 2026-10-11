//! Directory naming: date prefixes, whitespace handling and collision resolution.

use chrono::Local;
use std::fs;
use std::path::Path;

/// Today's date as `YYYY-MM-DD` in local time.
pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Length of a leading `YYYY-MM-DD-` prefix (11 bytes), if present.
fn date_prefix_len(name: &str) -> Option<usize> {
    let b = name.as_bytes();
    if b.len() < 11 {
        return None;
    }
    let ok = b[..11].iter().enumerate().all(|(i, c)| match i {
        4 | 7 | 10 => *c == b'-',
        _ => c.is_ascii_digit(),
    });
    ok.then_some(11)
}

pub fn has_date_prefix(name: &str) -> bool {
    date_prefix_len(name).is_some()
}

/// `2025-08-17-redis` -> `redis`; names without a date prefix are returned unchanged.
pub fn strip_date_prefix(name: &str) -> &str {
    &name[date_prefix_len(name).unwrap_or(0)..]
}

/// Collapse whitespace runs into single dashes (`"my idea"` -> `"my-idea"`).
pub fn dashify(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Turn user input into a single safe path component.
pub fn sanitize_name(s: &str) -> String {
    dashify(s).replace('/', "-")
}

fn taken(path: &Path) -> bool {
    // symlink_metadata: a dangling symlink or plain file also blocks mkdir.
    fs::symlink_metadata(path).is_ok()
}

/// Pick a name (without date prefix) that does not collide with an existing
/// `<date>-<name>` directory under `tries`.
///
/// If `base` ends in digits the number is bumped (`feature1` -> `feature2`);
/// otherwise `-2`, `-3`, ... is appended.
pub fn resolve_unique_name(tries: &Path, date: &str, base: &str) -> String {
    let free = |b: &str| !taken(&tries.join(format!("{date}-{b}")));
    if free(base) {
        return base.to_string();
    }

    let stem_len = base.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, digits) = base.split_at(stem_len);
    if let Ok(n) = digits.parse::<u64>() {
        let mut k = n;
        while let Some(next) = k.checked_add(1) {
            k = next;
            let cand = format!("{stem}{k}");
            if free(&cand) {
                return cand;
            }
        }
    }

    (2u64..)
        .map(|i| format!("{base}-{i}"))
        .find(|c| free(c))
        .expect("u64 suffix space exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_prefix_detection() {
        assert!(has_date_prefix("2025-08-17-redis"));
        assert!(has_date_prefix("2025-08-17-"));
        assert!(!has_date_prefix("2025-08-17"));
        assert!(!has_date_prefix("25-08-17-redis"));
        assert!(!has_date_prefix("2025_08_17-redis"));
        assert_eq!(strip_date_prefix("2025-08-17-redis"), "redis");
        assert_eq!(strip_date_prefix("redis"), "redis");
    }

    #[test]
    fn sanitizing() {
        assert_eq!(dashify("  my   cool\tidea "), "my-cool-idea");
        assert_eq!(sanitize_name("a/b c"), "a-b-c");
    }

    #[test]
    fn unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path();
        assert_eq!(resolve_unique_name(t, "2025-01-01", "feat1"), "feat1");

        fs::create_dir(t.join("2025-01-01-feat1")).unwrap();
        assert_eq!(resolve_unique_name(t, "2025-01-01", "feat1"), "feat2");

        fs::create_dir(t.join("2025-01-01-feat2")).unwrap();
        assert_eq!(resolve_unique_name(t, "2025-01-01", "feat1"), "feat3");

        fs::create_dir(t.join("2025-01-01-plain")).unwrap();
        assert_eq!(resolve_unique_name(t, "2025-01-01", "plain"), "plain-2");
        fs::create_dir(t.join("2025-01-01-plain-2")).unwrap();
        assert_eq!(resolve_unique_name(t, "2025-01-01", "plain"), "plain-3");

        // Overflowing digit runs fall back to the -N scheme instead of panicking.
        let huge = "x99999999999999999999999999";
        fs::create_dir(t.join(format!("2025-01-01-{huge}"))).unwrap();
        assert_eq!(
            resolve_unique_name(t, "2025-01-01", huge),
            format!("{huge}-2")
        );
    }
}
