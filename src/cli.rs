//! Argument parsing and command dispatch.
//!
//! Contract with the shell wrapper: on success a script goes to **stdout**
//! (exit 0) to be `eval`ed; on cancel `Cancelled.` goes to stdout (exit 1) to be
//! echoed; errors, help and version go to **stderr**.

use crate::config::{self, Config, DeleteMode, Settings};
use crate::doctor;
use crate::git;
use crate::init::{self, Installed, Shell};
use crate::naming;
use crate::paths::{absolutize, home};
use crate::script::{self, Script, TermHost};
use crate::trash;
use crate::tui::keys;
use crate::tui::selector::{Outcome, Selector};
use crate::tui::style::Style;
use crate::tui::term::{self, Hooks};
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
try - ephemeral workspace manager

To use try, add to your shell config:

  # bash/zsh (~/.bashrc or ~/.zshrc)
  eval \"$(try init ~/src/tries)\"

  # fish (~/.config/fish/config.fish)
  try init ~/src/tries | source

Usage:
  try [query]                 Interactive directory selector
  try clone <url> [name]      Clone repo into dated directory
  try worktree [dir] [name]   Create worktree from a git repo (default: cwd)
  try . <name>                Worktree of the current repo (plain dir outside git)
  try ./path/to/repo [name]   Worktree of another repo
  try --help                  Show this help

Commands:
  init [path]                 Output shell function definition
  install [path]              Add the shell function to your shell rc file
  clone <url> [name]          Clone git repo into date-prefixed directory
  worktree [dir] [name]       Create worktree in dated directory
  undo                        Restore what Ctrl-D last moved to the trash
  trash [list|empty]          Inspect the trash; `empty [--older-than DAYS] --yes` purges it
  doctor                      Check that the installation works
  config [init]               Show effective settings; `init` writes a starter config file

Examples:
  try                         Open interactive selector
  try project                 Selector with initial filter
  try clone https://github.com/user/repo
  try https://github.com/user/repo/pull/123
  try . feature-branch

Manual mode (without the shell function):
  try exec [query]            Output shell script to eval

Options:
  --path <dir>                Tries directory for this call
  --no-colors                 Disable colors (also: NO_COLOR=1)

Environment:
  TRY_PATH                    Tries directory (default: ~/src/tries)
  TRY_PROJECTS                Graduate destination (default: parent of TRY_PATH)
  TRY_DELETE                  trash (default) or remove (rm -rf) for Ctrl-D
  TRY_GIT_STATUS              0 hides the git markers in the list
  Settings can also live in ~/.config/try/config.toml (see `try config`).

Keyboard:
  Up/Down, Ctrl-P/N, Ctrl-K/J Navigate
  Enter                       Select / Create new
  Ctrl-R                      Rename
  Ctrl-G                      Graduate (promote try to project)
  Ctrl-D                      Mark for deletion (Enter, then type YES); recoverable via `try undo`
  Ctrl-T                      Create new try
  Esc                         Cancel
";

/// What a command wants printed.
enum Reply {
    Script(Script),
    Text(String),
    Cancelled,
    Usage,
    /// Already reported on stderr; just exit with this code.
    Exit(i32),
}

type CmdResult = Result<Reply, String>;

struct Ctx {
    tries: PathBuf,
    settings: Settings,
    config: Result<Config, String>,
    host: TermHost,
    style: Style,
    date: String,
    hooks: Hooks,
    initial_input: Option<String>,
}

/// Remove every `name` flag; true if any was present.
fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != name);
    args.len() != before
}

/// Remove every `--name VALUE` / `--name=VALUE`; the last one wins.
fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let eq = format!("{name}=");
    let mut value = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            args.remove(i);
            if i < args.len() {
                value = Some(args.remove(i));
            }
        } else if let Some(v) = args[i].strip_prefix(&eq) {
            value = Some(v.to_string());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    value
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

/// Entry point. Returns the process exit code.
pub fn run(mut args: Vec<String>) -> i32 {
    let no_color_flag =
        take_flag(&mut args, "--no-colors") | take_flag(&mut args, "--no-expand-tokens");

    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprint!("{HELP}");
        return 0;
    }
    if args.iter().any(|a| a == "--version" || a == "-v") {
        eprintln!("try {VERSION}");
        return 0;
    }

    let path_flag = take_opt(&mut args, "--path");
    let config = config::load();
    let cfg = config.clone().unwrap_or_default();
    let settings = match Settings::resolve(&cfg, path_flag.as_deref(), no_color_flag, &env_nonempty)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    let and_keys = take_opt(&mut args, "--and-keys");
    let hooks = Hooks {
        keys: and_keys
            .as_deref()
            .map(keys::parse_spec)
            .unwrap_or_default(),
        and_exit: take_flag(&mut args, "--and-exit"),
        confirm: take_opt(&mut args, "--and-confirm"),
    };
    let ctx = Ctx {
        tries: settings.tries.clone(),
        style: Style {
            color: settings.color,
        },
        settings,
        config,
        host: TermHost::from_env(),
        date: naming::today(),
        hooks,
        initial_input: take_opt(&mut args, "--and-type"),
    };

    match dispatch(args, ctx) {
        Ok(Reply::Script(s)) => emit(&s.render(), 0),
        Ok(Reply::Text(t)) => emit(&t, 0),
        Ok(Reply::Cancelled) => emit("Cancelled.\n", 1),
        Ok(Reply::Usage) => {
            eprint!("{HELP}");
            2
        }
        Ok(Reply::Exit(code)) => code,
        Err(msg) => {
            eprintln!("Error: {msg}");
            1
        }
    }
}

fn emit(text: &str, code: i32) -> i32 {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|_| out.flush());
    code
}

fn dispatch(mut args: Vec<String>, ctx: Ctx) -> CmdResult {
    let exec = args.first().map(String::as_str) == Some("exec");
    if exec {
        args.remove(0);
    }
    let command = args.first().map(String::as_str);
    // A broken config blocks everything except the commands that diagnose it.
    if let Err(e) = &ctx.config {
        if !matches!(command, Some("doctor" | "config")) {
            return Err(e.clone());
        }
    }
    match command {
        // Bare `try` without the wrapper function: usage error.
        None if !exec => Ok(Reply::Usage),
        None => cmd_cd(Vec::new(), ctx),
        Some("clone") => cmd_clone(&args[1..], &ctx),
        Some("init") if !exec => cmd_init(&args[1..], &ctx),
        Some("install") if !exec => cmd_install(&args[1..], &ctx),
        Some("worktree") => cmd_worktree(&args[1..], &ctx),
        Some("undo") => cmd_undo(&ctx),
        Some("trash") => cmd_trash(args[1..].to_vec(), &ctx),
        Some("doctor") => cmd_doctor(&ctx),
        Some("config") => cmd_config(&args[1..], &ctx),
        Some("cd") => cmd_cd(args[1..].to_vec(), ctx),
        Some(_) => cmd_cd(args, ctx),
    }
}

/// `YYYY-MM-DD-<name>` under the tries root, de-duplicated against today's dirs.
fn dated_dir(ctx: &Ctx, repo: &Path, custom: &str) -> PathBuf {
    let base = if custom.trim().is_empty() {
        let real = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
        real.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "try".into())
    } else {
        naming::sanitize_name(custom)
    };
    let unique = naming::resolve_unique_name(&ctx.tries, &ctx.date, &base);
    ctx.tries.join(format!("{}-{unique}", ctx.date))
}

fn cmd_clone(args: &[String], ctx: &Ctx) -> CmdResult {
    let url = args
        .first()
        .ok_or("git URI required for clone command\nUsage: try clone <git-uri> [name]")?;
    clone_into(ctx, url, args.get(1).map(String::as_str))
}

fn clone_into(ctx: &Ctx, url: &str, name: Option<&str>) -> CmdResult {
    let dir = git::clone_dir_name(url, name, &ctx.date)
        .ok_or_else(|| format!("Unable to parse git URI: {url}"))?;
    let path = ctx.tries.join(dir);
    Ok(Reply::Script(match git::github_pr(url) {
        Some(pr) => script::clone_pr(&path, &pr.clone_url, &pr.number, &ctx.host),
        None => script::clone(&path, url, &ctx.host),
    }))
}

/// Does the first `worktree` argument name a repository rather than a branch?
fn looks_like_repo_arg(a: &str) -> bool {
    matches!(a, "dir" | "." | "..") || a.starts_with('~') || a.contains('/')
}

fn cmd_worktree(args: &[String], ctx: &Ctx) -> CmdResult {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let (repo, rest) = match args.split_first() {
        Some((first, rest)) if looks_like_repo_arg(first) => (
            if first == "dir" {
                cwd
            } else {
                absolutize(first)
            },
            rest,
        ),
        _ => (cwd, args),
    };
    let path = dated_dir(ctx, &repo, &rest.join(" "));
    Ok(Reply::Script(script::worktree(&path, &repo, &ctx.host)))
}

/// A path to bake into the wrapper: `try init <path>` or `try init --path <path>`.
fn explicit_path(args: &[String], ctx: &Ctx) -> Option<PathBuf> {
    match args.first() {
        Some(p) => Some(absolutize(p)),
        None if ctx.settings.tries_source == config::Source::Flag => Some(ctx.tries.clone()),
        None => None,
    }
}

fn cmd_init(args: &[String], ctx: &Ctx) -> CmdResult {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let explicit = explicit_path(args, ctx);
    let shell = Shell::detect().unwrap_or(Shell::Bash);
    Ok(Reply::Text(init::snippet(shell, &exe, explicit.as_deref())))
}

fn cmd_install(args: &[String], ctx: &Ctx) -> CmdResult {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let explicit = explicit_path(args, ctx);
    let shell = Shell::detect().ok_or(
        "could not determine your shell\nRun 'try init' and manually add the output to your shell config.",
    )?;
    let home = home().ok_or("HOME is not set")?;
    let snippet = init::snippet(shell, &exe, explicit.as_deref());
    match init::install(shell, &home, &snippet).map_err(|e| {
        format!("{e}\nRun 'try init' and manually add the output to your shell config.")
    })? {
        Installed::Added(rc) => {
            eprintln!("Added try shell integration to {}", rc.display());
            eprintln!("Restart your shell or run: source {}", rc.display());
        }
        Installed::AlreadyPresent(rc) => {
            eprintln!("try is already installed in {}", rc.display());
            eprintln!("To reinstall, remove the '{}' block first.", init::MARKER);
        }
    }
    Ok(Reply::Text(String::new()))
}

fn cmd_undo(ctx: &Ctx) -> CmdResult {
    let batches = trash::batches(&ctx.tries).map_err(|e| e.to_string())?;
    let latest = batches
        .last()
        .filter(|b| !b.names.is_empty())
        .ok_or("Nothing to undo: the trash is empty")?;
    let items = trash::plan_restore(&ctx.tries, &latest.names);
    Ok(Reply::Script(script::restore(
        &ctx.tries, &latest.id, &items, &ctx.host,
    )))
}

fn days(n: u64) -> Duration {
    Duration::from_secs(n.saturating_mul(86_400))
}

fn cmd_trash(mut args: Vec<String>, ctx: &Ctx) -> CmdResult {
    let yes = take_flag(&mut args, "--yes");
    let older_than = take_opt(&mut args, "--older-than")
        .map(|v| {
            v.parse::<u64>()
                .map(days)
                .map_err(|_| format!("--older-than expects a number of days, got \"{v}\""))
        })
        .transpose()?;
    let now = SystemTime::now();

    match args.first().map(String::as_str) {
        None | Some("list") => {
            let batches = trash::batches(&ctx.tries).map_err(|e| e.to_string())?;
            if batches.is_empty() {
                eprintln!("Trash is empty ({})", trash::dir(&ctx.tries).display());
            }
            for b in batches.iter().rev() {
                let age = b.age(now).as_secs();
                let when = match age {
                    0..=59 => "just now".to_string(),
                    60..=3599 => format!("{}m ago", age / 60),
                    3600..=86_399 => format!("{}h ago", age / 3600),
                    _ => format!("{}d ago", age / 86_400),
                };
                eprintln!("{when:>9}  {}", b.names.join(", "));
            }
            Ok(Reply::Exit(0))
        }
        Some("empty") => {
            let doomed = trash::expired(&ctx.tries, older_than, now).map_err(|e| e.to_string())?;
            let items: usize = doomed.iter().map(|b| b.names.len()).sum();
            if doomed.is_empty() {
                eprintln!("Nothing to purge.");
                return Ok(Reply::Exit(0));
            }
            if !yes {
                return Err(format!(
                    "this would permanently delete {items} item(s) from {} batch(es); re-run with --yes",
                    doomed.len()
                ));
            }
            let (nb, ni) = trash::empty(&ctx.tries, older_than, now).map_err(|e| e.to_string())?;
            eprintln!("Permanently deleted {ni} item(s) from {nb} batch(es).");
            Ok(Reply::Exit(0))
        }
        Some(other) => Err(format!("unknown trash command \"{other}\"\nUsage: try trash [list | empty [--older-than DAYS] --yes]")),
    }
}

fn cmd_doctor(ctx: &Ctx) -> CmdResult {
    let exe = env::current_exe().ok();
    let home = home();
    let config_file = config::file_path();
    let checks = doctor::run(&doctor::Inputs {
        settings: &ctx.settings,
        config_file: config_file.as_deref(),
        config: &ctx.config,
        home: home.as_deref(),
        exe: exe.as_deref(),
        shell: Shell::detect(),
    });
    for c in &checks {
        eprintln!("{c}");
    }
    if doctor::healthy(&checks) {
        eprintln!("\nAll good.");
        Ok(Reply::Exit(0))
    } else {
        eprintln!("\nProblems found (FAIL above).");
        Ok(Reply::Exit(1))
    }
}

fn cmd_config(args: &[String], ctx: &Ctx) -> CmdResult {
    let file = config::file_path().ok_or("cannot locate a config directory (HOME is not set)")?;
    if args.first().map(String::as_str) == Some("init") {
        if file.exists() {
            return Err(format!("{} already exists", file.display()));
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, config::TEMPLATE).map_err(|e| e.to_string())?;
        eprintln!("Wrote {}", file.display());
        return Ok(Reply::Exit(0));
    }

    let s = &ctx.settings;
    eprintln!(
        "config file: {} ({})",
        file.display(),
        match &ctx.config {
            _ if !file.exists() => "not found; `try config init` creates one".to_string(),
            Ok(_) => "valid".to_string(),
            Err(e) => format!("INVALID: {e}"),
        }
    );
    eprintln!("path      {}  [{}]", s.tries.display(), s.tries_source);
    eprintln!(
        "projects  {}  [{}]",
        s.projects.display(),
        s.projects_source
    );
    eprintln!("delete    {}  [{}]", s.delete, s.delete_source);
    eprintln!(
        "git       {}  [{}]",
        if s.git_status { "on" } else { "off" },
        s.git_status_source
    );
    eprintln!("colors    {}", if s.color { "on" } else { "off" });
    Ok(Reply::Exit(0))
}

fn cmd_cd(mut args: Vec<String>, ctx: Ctx) -> CmdResult {
    if args.first().map(String::as_str) == Some("clone") {
        return cmd_clone(&args[1..], &ctx);
    }

    // `try .` / `try ./path [name]`: dated dir, worktree when the source is a repo.
    if args.first().is_some_and(|a| a.starts_with('.')) {
        let path_arg = args.remove(0);
        let custom = args.join(" ");
        if path_arg == "." && custom.trim().is_empty() {
            return Err("'try .' requires a name argument\nUsage: try . <name>".into());
        }
        let repo = absolutize(&path_arg);
        let path = dated_dir(&ctx, &repo, &custom);
        return Ok(Reply::Script(if repo.join(".git").exists() {
            script::worktree(&path, &repo, &ctx.host)
        } else {
            script::mkdir_cd(&path, &ctx.host)
        }));
    }

    // A bare git URL means "clone it".
    if args.first().is_some_and(|a| git::is_git_uri(a)) {
        let url = args.remove(0);
        let name = args.join(" ");
        return clone_into(&ctx, &url, Some(&name));
    }

    let query = args.join(" ");
    let initial = ctx.initial_input.as_deref().unwrap_or(&query);
    let mut selector = Selector::open(
        &ctx.tries,
        initial,
        ctx.settings.projects.clone(),
        ctx.style,
    )
    .map_err(|e| format!("cannot open {}: {e}", ctx.tries.display()))?;
    selector.set_trash(ctx.settings.delete == DeleteMode::Trash);
    selector.set_git_status(ctx.settings.git_status);
    let outcome = term::run(&mut selector, ctx.hooks).map_err(|e| e.to_string())?;
    Ok(match outcome {
        Outcome::Cancel => Reply::Cancelled,
        Outcome::Cd(path) => Reply::Script(script::cd(&path, &ctx.host)),
        Outcome::Mkdir(path) => Reply::Script(script::mkdir_cd(&path, &ctx.host)),
        Outcome::Rename { old, new } => {
            Reply::Script(script::rename(&ctx.tries, &old, &new, &ctx.host))
        }
        Outcome::Graduate { source, dest, name } => Reply::Script(script::graduate(
            &source, &dest, &ctx.tries, &name, &ctx.host,
        )),
        Outcome::Delete { base, names } => {
            let cwd = env::current_dir().unwrap_or_else(|_| base.clone());
            Reply::Script(match ctx.settings.delete {
                DeleteMode::Trash => script::trash(&base, &names, &cwd, &trash::new_batch_id()),
                DeleteMode::Remove => script::delete(&base, &names, &cwd),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn option_extraction_last_wins_and_both_forms() {
        let mut a = v(&["--path", "/a", "x", "--path=/b", "y"]);
        assert_eq!(take_opt(&mut a, "--path").as_deref(), Some("/b"));
        assert_eq!(a, v(&["x", "y"]));

        let mut a = v(&["x", "--path"]);
        assert_eq!(take_opt(&mut a, "--path"), None);
        assert_eq!(a, v(&["x"]));
    }

    #[test]
    fn flags_are_removed_everywhere() {
        let mut a = v(&["--no-colors", "q", "--no-colors"]);
        assert!(take_flag(&mut a, "--no-colors"));
        assert_eq!(a, v(&["q"]));
        assert!(!take_flag(&mut a, "--no-colors"));
    }

    #[test]
    fn worktree_first_arg_classification() {
        assert!(looks_like_repo_arg("dir"));
        assert!(looks_like_repo_arg("."));
        assert!(looks_like_repo_arg("/abs/repo"));
        assert!(looks_like_repo_arg("../x"));
        assert!(looks_like_repo_arg("~/code"));
        assert!(!looks_like_repo_arg("feature-x"));
    }
}
