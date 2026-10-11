//! Shell script generation.
//!
//! Everything `try` wants the parent shell to do is expressed as a list of
//! commands, rendered as a `&&`-chained POSIX-ish script that bash, zsh and
//! fish can all `eval`. All paths are single-quoted.

use std::path::Path;

pub const WARNING: &str =
    "# if you can read this, you didn't launch try from an alias. run try --help.";

/// Single-quote `s` for POSIX shells and fish.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn q(path: &Path) -> String {
    quote(&path.to_string_lossy())
}

/// Ordered shell commands, joined with `&&`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Script(pub Vec<String>);

impl Script {
    pub fn render(&self) -> String {
        let mut out = String::from(WARNING);
        out.push('\n');
        let last = self.0.len().saturating_sub(1);
        for (i, cmd) in self.0.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(cmd);
            out.push_str(if i < last { " && \\\n" } else { "\n" });
        }
        out
    }

    fn then(mut self, more: Vec<String>) -> Self {
        self.0.extend(more);
        self
    }
}

/// Terminal multiplexers that can be told what we are working on.
#[derive(Debug, Default, Clone)]
pub struct TermHost {
    pub herdr_env: Option<String>,
    pub herdr_pane: Option<String>,
    pub herdr_workspace: Option<String>,
    pub cmux_socket: Option<String>,
    pub cmux_bundle: Option<String>,
}

impl TermHost {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        TermHost {
            herdr_env: get("HERDR_ENV"),
            herdr_pane: get("HERDR_PANE_ID"),
            herdr_workspace: get("HERDR_WORKSPACE_ID"),
            cmux_socket: get("CMUX_SOCKET_PATH"),
            cmux_bundle: get("CMUX_BUNDLE_ID"),
        }
    }

    /// Best-effort, non-fatal commands that label the current pane/tab.
    /// They go last so a missing CLI can never block the `cd`.
    fn rename_commands(&self, dir: &Path) -> Vec<String> {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = format!("try: {}", crate::naming::strip_date_prefix(&name));

        if self.herdr_env.as_deref() == Some("1") {
            if let Some(pane) = &self.herdr_pane {
                let mut cmds = vec![format!(
                    "command -v herdr >/dev/null 2>&1 && herdr pane report-metadata {} --source try --title {} >/dev/null 2>&1 || true",
                    quote(pane),
                    quote(&label)
                )];
                if let (true, Some(ws)) = (pane.ends_with(":p1"), &self.herdr_workspace) {
                    cmds.push(format!(
                        "command -v herdr >/dev/null 2>&1 && herdr workspace rename {} {} >/dev/null 2>&1 || true",
                        quote(ws),
                        quote(&label)
                    ));
                }
                return cmds;
            }
        }
        if self.cmux_socket.is_some() || self.cmux_bundle.is_some() {
            return vec![format!(
                "command -v cmux >/dev/null 2>&1 && cmux rename-tab {} >/dev/null 2>&1 || true",
                quote(&label)
            )];
        }
        Vec::new()
    }
}

/// Visit an existing directory (touching it so it sorts first next time).
pub fn cd(path: &Path, host: &TermHost) -> Script {
    Script(vec![
        format!("touch {}", q(path)),
        format!("echo {}", q(path)),
        format!("cd {}", q(path)),
    ])
    .then(host.rename_commands(path))
}

pub fn mkdir_cd(path: &Path, host: &TermHost) -> Script {
    Script(vec![format!("mkdir -p {}", q(path))]).then(cd(path, host).0)
}

pub fn clone(path: &Path, url: &str, host: &TermHost) -> Script {
    Script(vec![
        format!("mkdir -p {}", q(path)),
        format!(
            "echo {}",
            quote(&format!("Using git clone to create this trial from {url}."))
        ),
        format!("git clone {} {}", quote(url), q(path)),
    ])
    .then(cd(path, host).0)
}

/// Clone `url`, fetch `pull/<n>/head` and check it out detached.
pub fn clone_pr(path: &Path, url: &str, number: &str, host: &TermHost) -> Script {
    let pull_ref = format!("pull/{number}/head");
    Script(vec![
        format!("mkdir -p {}", q(path)),
        format!(
            "echo {}",
            quote(&format!(
                "Using git clone to create this trial from {url} PR #{number}."
            ))
        ),
        format!("git clone {} {}", quote(url), q(path)),
        format!("git -C {} fetch origin {}", q(path), quote(&pull_ref)),
        format!("git -C {} checkout --detach FETCH_HEAD", q(path)),
    ])
    .then(cd(path, host).0)
}

/// Create `path` as a detached worktree of `repo`.
///
/// The worktree step runs inside `sh -c` so the `if` syntax is the same under
/// fish. If `repo` is not a git work tree the directory is simply created.
pub fn worktree(path: &Path, repo: &Path, host: &TermHost) -> Script {
    let r = q(repo);
    let inner = format!(
        "if git -C {r} rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
         top=$(git -C {r} rev-parse --show-toplevel) && \
         git -C \"$top\" worktree add --detach {p} >/dev/null || true; fi; exit 0",
        p = q(path)
    );
    Script(vec![
        format!("mkdir -p {}", q(path)),
        format!(
            "echo {}",
            quote(&format!(
                "Using git worktree to create this trial from {}.",
                repo.display()
            ))
        ),
        format!("/usr/bin/env sh -c {}", quote(&inner)),
    ])
    .then(cd(path, host).0)
}

/// Remove `names` (basenames directly under `base`), then return to where the
/// shell was — or to `base` if that directory just vanished.
pub fn delete(base: &Path, names: &[String], cwd: &Path) -> Script {
    let mut cmds = vec![format!("cd {}", q(base))];
    for n in names {
        // `./` keeps a name like `-rf` from being parsed as an option.
        let arg = quote(&if n.starts_with('-') {
            format!("./{n}")
        } else {
            n.clone()
        });
        cmds.push(format!("test -d {arg} && rm -rf {arg}"));
    }
    cmds.push(format!("cd {} 2>/dev/null || cd {}", q(cwd), q(base)));
    Script(cmds)
}

/// Move `names` (basenames directly under `base`) into `<base>/.trash/<batch>/`,
/// then return to where the shell was — or to `base` if that directory moved.
pub fn trash(base: &Path, names: &[String], cwd: &Path, batch: &str) -> Script {
    let dest = format!("{}/{batch}", crate::trash::DIR);
    let mut cmds = vec![
        format!("cd {}", q(base)),
        format!("mkdir -p {}", quote(&dest)),
    ];
    for n in names {
        let src = quote(&if n.starts_with('-') {
            format!("./{n}")
        } else {
            n.clone()
        });
        cmds.push(format!(
            "test -d {src} && mv -- {src} {}",
            quote(&format!("{dest}/{n}"))
        ));
    }
    cmds.push(format!(
        "echo {}",
        quote(&format!(
            "Moved to trash: {} (restore with: try undo)",
            names.join(", ")
        ))
    ));
    cmds.push(format!("cd {} 2>/dev/null || cd {}", q(cwd), q(base)));
    Script(cmds)
}

/// Move `items` (`(trashed name, restored name)`) from a trash batch back into
/// `base`. A single item is also visited.
pub fn restore(base: &Path, batch: &str, items: &[(String, String)], host: &TermHost) -> Script {
    let from = format!("{}/{batch}", crate::trash::DIR);
    let mut cmds = vec![format!("cd {}", q(base))];
    for (name, restored) in items {
        cmds.push(format!(
            "mv -- {} {}",
            quote(&format!("{from}/{name}")),
            quote(restored)
        ));
    }
    // Empty the batch directory; leave it if something unrestorable remains.
    cmds.push(format!("rmdir {} 2>/dev/null || true", quote(&from)));
    cmds.push(format!(
        "rmdir {} 2>/dev/null || true",
        quote(crate::trash::DIR)
    ));
    let names: Vec<&str> = items.iter().map(|(_, r)| r.as_str()).collect();
    cmds.push(format!(
        "echo {}",
        quote(&format!("Restored: {}", names.join(", ")))
    ));
    let mut script = Script(cmds);
    if let [(_, restored)] = items {
        script = script.then(cd(&base.join(restored), host).0);
    }
    script
}

pub fn rename(base: &Path, old: &str, new: &str, host: &TermHost) -> Script {
    let new_path = base.join(new);
    Script(vec![
        format!("cd {}", q(base)),
        format!("mv -- {} {}", quote(old), quote(new)),
        format!("echo {}", q(&new_path)),
        format!("cd {}", q(&new_path)),
    ])
    .then(host.rename_commands(&new_path))
}

/// Promote a try to a permanent project: move it, leave a symlink behind.
pub fn graduate(source: &Path, dest: &Path, base: &Path, name: &str, host: &TermHost) -> Script {
    // Worktrees have a `.git` *file*; `git worktree move` keeps git's books straight.
    let is_worktree = source.join(".git").is_file();
    let mv = if is_worktree {
        format!("git worktree move {} {}", q(source), q(dest))
    } else {
        format!("mv {} {}", q(source), q(dest))
    };
    Script(vec![
        mv,
        format!("ln -s {} {}", q(dest), q(&base.join(name))),
        format!(
            "echo {}",
            quote(&format!("Graduated: {name} → {}", dest.display()))
        ),
    ])
    .then(cd(dest, host).0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
        assert_eq!(quote("a b"), "'a b'");
    }

    #[test]
    fn render_format() {
        let s = Script(vec!["a".into(), "b".into(), "c".into()]).render();
        assert_eq!(s, format!("{WARNING}\na && \\\n  b && \\\n  c\n"));
        assert_eq!(
            Script(vec!["only".into()]).render(),
            format!("{WARNING}\nonly\n")
        );
    }

    #[test]
    fn herdr_and_cmux_labels_strip_date() {
        let p = PathBuf::from("/t/2025-01-02-redis");
        let herdr = TermHost {
            herdr_env: Some("1".into()),
            herdr_pane: Some("w1:p1".into()),
            herdr_workspace: Some("w1".into()),
            ..Default::default()
        };
        let cmds = herdr.rename_commands(&p);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains("--title 'try: redis'"));
        assert!(cmds[1].contains("workspace rename 'w1'"));

        let cmux = TermHost {
            cmux_socket: Some("/s".into()),
            ..Default::default()
        };
        assert!(cmux.rename_commands(&p)[0].contains("cmux rename-tab 'try: redis'"));
        assert!(TermHost::default().rename_commands(&p).is_empty());
    }

    #[test]
    fn urls_and_paths_are_quoted_in_clone() {
        let s = clone(Path::new("/t/it's"), "https://x/y z", &TermHost::default());
        let text = s.render();
        assert!(text.contains(r#"git clone 'https://x/y z' '/t/it'"'"'s'"#));
    }

    #[test]
    fn trash_moves_into_a_batch_and_returns() {
        let s = trash(
            Path::new("/b"),
            &["-x".into(), "it's".into()],
            Path::new("/w"),
            "007",
        );
        assert_eq!(s.0[0], "cd '/b'");
        assert_eq!(s.0[1], "mkdir -p '.trash/007'");
        assert_eq!(s.0[2], "test -d './-x' && mv -- './-x' '.trash/007/-x'");
        assert!(s.0[3].starts_with("test -d 'it'\"'\"'s' && mv -- "));
        assert!(s.0[4].starts_with("echo 'Moved to trash: -x, it"));
        assert_eq!(s.0.last().unwrap(), "cd '/w' 2>/dev/null || cd '/b'");
    }

    #[test]
    fn restore_visits_only_a_single_item() {
        let host = TermHost::default();
        let one = restore(
            Path::new("/b"),
            "007",
            &[("x".into(), "x-restored".into())],
            &host,
        );
        assert!(one
            .0
            .contains(&"mv -- '.trash/007/x' 'x-restored'".to_string()));
        assert_eq!(one.0.last().unwrap(), "cd '/b/x-restored'");
        let two = restore(
            Path::new("/b"),
            "007",
            &[("x".into(), "x".into()), ("y".into(), "y".into())],
            &host,
        );
        assert!(two.0.iter().all(|c| !c.starts_with("cd '/b/")));
    }

    #[test]
    fn delete_returns_to_cwd_or_base() {
        let s = delete(
            Path::new("/b"),
            &["-rf".into(), "x".into()],
            Path::new("/w"),
        );
        assert_eq!(
            s.0,
            vec![
                "cd '/b'",
                "test -d './-rf' && rm -rf './-rf'",
                "test -d 'x' && rm -rf 'x'",
                "cd '/w' 2>/dev/null || cd '/b'"
            ]
        );
    }
}
