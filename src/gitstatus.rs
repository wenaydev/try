//! Cheap, bounded `git status` probing for the list markers.
//!
//! One `git status --porcelain=v2 --branch` per repository. We stop reading as
//! soon as the first changed path shows up (the answer is already "dirty"), and
//! a watchdog kills git if it hangs, so a huge or stalled repository can never
//! block the UI. Probes run on worker threads; see [`Worker`].

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GitStatus {
    /// Modified, staged or untracked files exist.
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    NotRepo,
    Repo(GitStatus),
    /// git missing, timed out, or refused (e.g. "dubious ownership").
    Failed,
}

/// Result of scanning git's output.
struct Scan {
    status: GitStatus,
    /// Hit a changed-path line and stopped early.
    saw_entry: bool,
}

/// Read branch headers, stopping at the first changed path.
fn scan(reader: impl BufRead) -> Scan {
    let mut status = GitStatus::default();
    for line in reader.split(b'\n') {
        let Ok(line) = line else { break };
        if let Some(_up) = line.strip_prefix(b"# branch.upstream ") {
            status.has_upstream = true;
        } else if let Some(ab) = line.strip_prefix(b"# branch.ab ") {
            let ab = String::from_utf8_lossy(ab);
            let mut it = ab.split_whitespace();
            let num = |s: Option<&str>, sign: char| {
                s.and_then(|s| s.strip_prefix(sign))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0)
            };
            status.ahead = num(it.next(), '+');
            status.behind = num(it.next(), '-');
        } else if !line.starts_with(b"#") && !line.is_empty() {
            status.dirty = true;
            return Scan {
                status,
                saw_entry: true,
            };
        }
    }
    Scan {
        status,
        saw_entry: false,
    }
}

pub fn probe(path: &Path) -> Probe {
    probe_with("git", path, TIMEOUT)
}

pub fn probe_with(git: &str, path: &Path, timeout: Duration) -> Probe {
    // `.git` is a directory in clones and a file in worktrees.
    if fs::symlink_metadata(path.join(".git")).is_err() {
        return Probe::NotRepo;
    }
    let spawned = Command::new(git)
        .args(["--no-optional-locks", "-C"])
        .arg(path)
        .args(["status", "--porcelain=v2", "--branch"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return Probe::Failed;
    };
    let stdout = child.stdout.take().expect("stdout is piped");
    let child = Arc::new(Mutex::new(child));

    // Watchdog: kill git if it outlives `timeout`; released early when we finish.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog = {
        let child = Arc::clone(&child);
        thread::spawn(move || {
            if done_rx.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                let _ = child.lock().map(|mut c| c.kill());
            }
        })
    };

    // Dropping the reader closes the pipe, so git gets SIGPIPE after an early stop.
    let scanned = scan(BufReader::new(stdout));
    let exit = child.lock().ok().and_then(|mut c| c.wait().ok());
    drop(done_tx);
    let _ = watchdog.join();

    if scanned.saw_entry || exit.is_some_and(|s| s.success()) {
        Probe::Repo(scanned.status)
    } else {
        Probe::Failed
    }
}

/// A small pool that probes repositories off the UI thread.
pub struct Worker {
    requests: mpsc::Sender<(String, PathBuf)>,
    results: mpsc::Receiver<(String, Probe)>,
}

impl Worker {
    pub fn spawn(threads: usize) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(String, PathBuf)>();
        let (res_tx, res_rx) = mpsc::channel();
        let req_rx = Arc::new(Mutex::new(req_rx));
        for _ in 0..threads.max(1) {
            let (req_rx, res_tx) = (Arc::clone(&req_rx), res_tx.clone());
            thread::spawn(move || loop {
                let job = req_rx.lock().map(|rx| rx.recv());
                let Ok(Ok((name, path))) = job else { break };
                if res_tx.send((name, probe(&path))).is_err() {
                    break;
                }
            });
        }
        Worker {
            requests: req_tx,
            results: res_rx,
        }
    }

    pub fn submit(&self, name: String, path: PathBuf) {
        let _ = self.requests.send((name, path));
    }

    /// Everything finished since the last call.
    pub fn drain(&self) -> Vec<(String, Probe)> {
        self.results.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::process::Command;

    fn scan_str(s: &str) -> Scan {
        scan(Cursor::new(s.as_bytes().to_vec()))
    }

    #[test]
    fn parses_branch_headers() {
        let s = scan_str(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n",
        );
        assert_eq!(
            s.status,
            GitStatus {
                dirty: false,
                ahead: 2,
                behind: 1,
                has_upstream: true
            }
        );
        assert!(!s.saw_entry);
    }

    #[test]
    fn first_entry_means_dirty_and_stops() {
        let s = scan_str("# branch.head main\n1 .M N... 100644 100644 100644 a b file\n? other\n");
        assert!(s.status.dirty && s.saw_entry);
        assert!(!s.status.has_upstream);
        // Untracked-only is dirty too, and non-UTF-8 paths do not break the scan.
        let mut raw = b"# branch.head main\n? na".to_vec();
        raw.extend([0xff, 0xfe, b'\n']);
        assert!(scan(Cursor::new(raw)).status.dirty);
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn real_repositories() {
        let t = tempfile::tempdir().unwrap();
        let plain = t.path().join("plain");
        fs::create_dir(&plain).unwrap();
        assert_eq!(probe(&plain), Probe::NotRepo);

        let repo = t.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let Probe::Repo(g) = probe(&repo) else {
            panic!("not a repo")
        };
        assert!(!g.dirty && !g.has_upstream && g.ahead == 0);

        fs::write(repo.join("new.txt"), "x").unwrap();
        let Probe::Repo(g) = probe(&repo) else {
            panic!()
        };
        assert!(g.dirty, "untracked files count as dirty");
    }

    #[test]
    fn ahead_of_upstream_and_worktrees() {
        let t = tempfile::tempdir().unwrap();
        let bare = t.path().join("origin.git");
        fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare"]);
        let clone = t.path().join("clone");
        git(t.path(), &["clone", "-q", bare.to_str().unwrap(), "clone"]);
        git(&clone, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&clone, &["push", "-q", "-u", "origin", "HEAD"]);
        git(&clone, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let Probe::Repo(g) = probe(&clone) else {
            panic!()
        };
        assert_eq!(
            (g.ahead, g.behind, g.has_upstream, g.dirty),
            (1, 0, true, false)
        );

        // `.git` is a file in a worktree.
        let wt = t.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        assert!(wt.join(".git").is_file());
        assert!(matches!(probe(&wt), Probe::Repo(_)));
    }

    #[cfg(unix)]
    #[test]
    fn hung_git_is_killed_by_the_watchdog() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let stub = t.path().join("slowgit");
        fs::write(&stub, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let r = probe_with(stub.to_str().unwrap(), &repo, Duration::from_millis(200));
        assert_eq!(r, Probe::Failed);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn missing_git_binary_fails_softly() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".git")).unwrap();
        assert_eq!(
            probe_with("/nonexistent/git", t.path(), TIMEOUT),
            Probe::Failed
        );
    }

    #[test]
    fn worker_delivers_results() {
        let t = tempfile::tempdir().unwrap();
        let w = Worker::spawn(2);
        w.submit("a".into(), t.path().to_path_buf());
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(w.drain());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, vec![("a".to_string(), Probe::NotRepo)]);
    }
}
