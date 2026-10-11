//! Shell integration: the wrapper function and `try install`.
//!
//! A child process cannot `cd` its parent, so `try init` prints a function that
//! runs `try exec ...`, then `eval`s the script it prints.

use crate::script::quote;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const MARKER: &str = "# try shell integration";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    /// Best guess from `$SHELL`, falling back to the parent process name.
    pub fn detect() -> Option<Shell> {
        let from = |s: &str| {
            if s.contains("fish") {
                Some(Shell::Fish)
            } else if s.contains("zsh") {
                Some(Shell::Zsh)
            } else if s.contains("bash") {
                Some(Shell::Bash)
            } else {
                None
            }
        };
        if let Some(sh) = std::env::var("SHELL").ok().and_then(|s| from(&s)) {
            return Some(sh);
        }
        parent_process_name().and_then(|n| from(&n))
    }

    pub fn rc_file(self, home: &Path) -> PathBuf {
        match self {
            Shell::Fish => home.join(".config/fish/config.fish"),
            Shell::Zsh => home.join(".zshrc"),
            Shell::Bash => {
                let rc = home.join(".bashrc");
                if rc.exists() {
                    rc
                } else {
                    home.join(".bash_profile")
                }
            }
        }
    }
}

#[cfg(unix)]
fn parent_process_name() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    let out = std::process::Command::new("ps")
        .args(["c", "-p", &ppid.to_string(), "-o", "ucomm="])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(not(unix))]
fn parent_process_name() -> Option<String> {
    None
}

/// The wrapper function for `shell`.
///
/// With `explicit_path` that directory is baked in. Otherwise the function
/// passes `--path "$TRY_PATH"` only when that variable is set (exported or not)
/// and lets the binary resolve config/default at call time, so editing
/// `config.toml` takes effect immediately.
pub fn snippet(shell: Shell, exe: &Path, explicit_path: Option<&Path>) -> String {
    let exe = quote(&exe.to_string_lossy());
    match (shell, explicit_path) {
        (Shell::Fish, Some(p)) => format!(
            "function try\n  \
             set -l out ({exe} exec --path {} $argv 2>/dev/tty | string collect)\n  \
             {FISH_TAIL}",
            quote(&p.to_string_lossy())
        ),
        (Shell::Fish, None) => format!(
            "function try\n  \
             set -l path_args\n  \
             if set -q TRY_PATH; and test -n \"$TRY_PATH\"\n    \
             set path_args --path \"$TRY_PATH\"\n  \
             end\n  \
             set -l out ({exe} exec $path_args $argv 2>/dev/tty | string collect)\n  \
             {FISH_TAIL}"
        ),
        (_, Some(p)) => format!(
            "try() {{\n  \
             local out\n  \
             out=$({exe} exec --path {} \"$@\" 2>/dev/tty)\n  \
             {POSIX_TAIL}",
            quote(&p.to_string_lossy())
        ),
        (_, None) => format!(
            "try() {{\n  \
             local out\n  \
             if [ -n \"$TRY_PATH\" ]; then\n    \
             out=$({exe} exec --path \"$TRY_PATH\" \"$@\" 2>/dev/tty)\n  \
             else\n    \
             out=$({exe} exec \"$@\" 2>/dev/tty)\n  \
             fi\n  \
             {POSIX_TAIL}"
        ),
    }
}

const POSIX_TAIL: &str = "if [ $? -eq 0 ]; then\n    \
     eval \"$out\"\n  \
     else\n    \
     echo \"$out\"\n  \
     fi\n\
     }\n";

const FISH_TAIL: &str = "if test $pipestatus[1] -eq 0\n    \
     eval $out\n  \
     else\n    \
     echo $out\n  \
     end\n\
     end\n";

pub enum Installed {
    Added(PathBuf),
    AlreadyPresent(PathBuf),
}

/// Append the wrapper to the shell's rc file (idempotent via [`MARKER`]).
pub fn install(shell: Shell, home: &Path, snippet: &str) -> io::Result<Installed> {
    let rc = shell.rc_file(home);
    let existing = fs::read_to_string(&rc).unwrap_or_default();
    if existing.contains(MARKER) {
        return Ok(Installed::AlreadyPresent(rc));
    }
    if let Some(parent) = rc.parent() {
        fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&rc)?;
    write!(f, "\n{MARKER}\n{snippet}")?;
    Ok(Installed::Added(rc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_wrapper_defers_to_the_binary_unless_try_path_is_set() {
        let s = snippet(Shell::Bash, Path::new("/usr/bin/try"), None);
        assert!(s.contains("out=$('/usr/bin/try' exec --path \"$TRY_PATH\" \"$@\" 2>/dev/tty)"));
        assert!(s.contains("out=$('/usr/bin/try' exec \"$@\" 2>/dev/tty)"));
        assert!(s.contains("eval \"$out\""));
        assert_eq!(snippet(Shell::Zsh, Path::new("/usr/bin/try"), None), s);
    }

    #[test]
    fn explicit_path_is_baked_in_and_quoted() {
        let s = snippet(
            Shell::Zsh,
            Path::new("/b/try"),
            Some(Path::new("/my tries")),
        );
        assert!(s.contains("--path '/my tries'"));
        assert!(!s.contains("TRY_PATH"));
        let f = snippet(
            Shell::Fish,
            Path::new("/b/try"),
            Some(Path::new("/my tries")),
        );
        assert!(f.contains("--path '/my tries' $argv"));
    }

    #[test]
    fn fish_snippet() {
        let s = snippet(Shell::Fish, Path::new("/b/try"), None);
        assert!(s.starts_with("function try\n"));
        assert!(s.contains("string collect"));
        assert!(s.contains("set path_args --path \"$TRY_PATH\""));
        assert!(s.contains("exec $path_args $argv"));
        assert!(s.trim_end().ends_with("end"));
    }

    #[test]
    fn install_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let snip = "try() { :; }\n";
        assert!(matches!(
            install(Shell::Zsh, home.path(), snip).unwrap(),
            Installed::Added(_)
        ));
        assert!(matches!(
            install(Shell::Zsh, home.path(), snip).unwrap(),
            Installed::AlreadyPresent(_)
        ));
        let rc = fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert_eq!(rc.matches(MARKER).count(), 1);
        assert!(rc.contains(snip));
    }

    #[test]
    fn bash_prefers_existing_bashrc() {
        let home = tempfile::tempdir().unwrap();
        assert!(Shell::Bash.rc_file(home.path()).ends_with(".bash_profile"));
        fs::write(home.path().join(".bashrc"), "").unwrap();
        assert!(Shell::Bash.rc_file(home.path()).ends_with(".bashrc"));
    }
}
