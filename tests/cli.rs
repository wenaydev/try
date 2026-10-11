//! Black-box tests: run the real binary and, where it matters, execute the
//! script it prints in a real shell.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// The binary, isolated from the developer's real environment and config.
fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_try"));
    c.env_remove("HERDR_ENV")
        .env_remove("HERDR_PANE_ID")
        .env_remove("CMUX_SOCKET_PATH")
        .env_remove("CMUX_BUNDLE_ID")
        .env_remove("TRY_PATH")
        .env_remove("TRY_PROJECTS")
        .env_remove("TRY_DELETE")
        .env("XDG_CONFIG_HOME", "/nonexistent-try-config");
    c
}

fn try_cmd(tries: &Path, args: &[&str]) -> Output {
    try_with(tries, &[], args)
}

fn try_with(tries: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    bin()
        .envs(env.iter().copied())
        .env("NO_COLOR", "1")
        .env("TRY_WIDTH", "80")
        .env("TRY_HEIGHT", "24")
        .arg("--path")
        .arg(tries)
        .args(args)
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn sandbox() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let tries = root.path().join("tries");
    fs::create_dir_all(tries.join("2025-11-01-alpha")).unwrap();
    fs::create_dir_all(tries.join("2025-11-15-beta")).unwrap();
    fs::create_dir_all(tries.join("2025-11-20-gamma")).unwrap();
    (root, tries)
}

fn sh(script: &str, cwd: &Path) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .unwrap()
}

#[test]
fn selecting_by_query_emits_cd_script() {
    let (_root, tries) = sandbox();
    let o = try_cmd(&tries, &["exec", "--and-keys=bet,ENTER"]);
    assert_eq!(o.status.code(), Some(0));
    let s = out(&o);
    assert!(s.contains("cd '") && s.contains("2025-11-15-beta'"), "{s}");
    assert!(s.starts_with("# if you can read this"));
    assert!(s.contains("touch '"));
}

#[test]
fn escape_and_exhausted_keys_cancel_with_exit_1() {
    let (_root, tries) = sandbox();
    for keys in ["--and-keys=ESC", "--and-keys=al"] {
        let o = try_cmd(&tries, &["exec", keys]);
        assert_eq!(o.status.code(), Some(1), "{keys}");
        assert_eq!(out(&o), "Cancelled.\n");
    }
}

#[test]
fn no_match_enter_creates_dated_directory() {
    let (_root, tries) = sandbox();
    let o = try_cmd(&tries, &["exec", "--and-keys=zzz,ENTER"]);
    let s = out(&o);
    let today = chrono_today();
    assert!(
        s.contains(&format!("mkdir -p '{}/{today}-zzz'", tries.display())),
        "{s}"
    );
}

fn chrono_today() -> String {
    try_cli::naming::today()
}

#[test]
fn and_exit_renders_one_frame_on_stderr() {
    let (_root, tries) = sandbox();
    let o = try_cmd(&tries, &["exec", "--and-exit"]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("Try Directory Selection") && err.contains("gamma"));
    assert!(!err.contains("\x1b[1;"), "NO_COLOR must suppress styling");
}

#[test]
fn bare_try_prints_usage_and_exits_2() {
    let (_root, tries) = sandbox();
    let o = try_cmd(&tries, &[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Usage:"));
    assert!(o.stdout.is_empty());
}

#[test]
fn help_and_version_go_to_stderr_so_eval_never_sees_them() {
    let (_root, tries) = sandbox();
    for flag in ["--help", "--version"] {
        let o = try_cmd(&tries, &["exec", flag]);
        assert_eq!(o.status.code(), Some(0));
        assert!(o.stdout.is_empty(), "{flag}");
        assert!(!o.stderr.is_empty());
    }
}

#[test]
fn clone_naming_and_quoting() {
    let (_root, tries) = sandbox();
    let today = chrono_today();

    let o = try_cmd(
        &tries,
        &["exec", "clone", "https://github.com/user/repo.git"],
    );
    let s = out(&o);
    assert!(
        s.contains(&format!(
            "git clone 'https://github.com/user/repo.git' '{}/{today}-user-repo'",
            tries.display()
        )),
        "{s}"
    );

    let o = try_cmd(&tries, &["https://github.com/user/repo", "my", "fork"]);
    assert!(out(&o).contains(&format!("{}/my-fork'", tries.display())));

    let o = try_cmd(
        &tries,
        &["exec", "clone", "https://github.com/tobi/try/pull/124"],
    );
    let s = out(&o);
    assert!(
        s.contains("fetch origin 'pull/124/head'") && s.contains("checkout --detach FETCH_HEAD"),
        "{s}"
    );
    assert!(s.contains(&format!("{today}-tobi-try'")));

    let o = try_cmd(&tries, &["exec", "clone"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("git URI required"));

    let o = try_cmd(&tries, &["exec", "clone", "nonsense"]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn dot_requires_a_name_and_versions_collisions() {
    let (root, tries) = sandbox();
    let today = chrono_today();
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();

    let run_in_repo = |args: &[&str]| {
        bin()
            .current_dir(&repo)
            .arg("--path")
            .arg(&tries)
            .args(args)
            .output()
            .unwrap()
    };

    let o = run_in_repo(&["exec", "."]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("requires a name"));

    fs::create_dir_all(tries.join(format!("{today}-feature1"))).unwrap();
    let s = out(&run_in_repo(&["exec", ".", "feature1"]));
    assert!(
        s.contains("worktree add") && s.contains(&format!("{today}-feature2'")),
        "{s}"
    );

    fs::create_dir_all(tries.join(format!("{today}-plain"))).unwrap();
    let s = out(&run_in_repo(&["exec", ".", "plain"]));
    assert!(s.contains(&format!("{today}-plain-2'")), "{s}");

    // Outside a git repo `try .` just makes a directory.
    let plain = root.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let o = bin()
        .current_dir(&plain)
        .arg("--path")
        .arg(&tries)
        .args(["exec", ".", "scratch"])
        .output()
        .unwrap();
    let s = out(&o);
    assert!(!s.contains("worktree add") && s.contains("mkdir -p"), "{s}");
}

#[test]
fn worktree_script_creates_a_real_worktree_in_a_real_repo() {
    let (root, tries) = sandbox();
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);

    let o = bin()
        .current_dir(&repo)
        .arg("--path")
        .arg(&tries)
        .args(["worktree", "wt"])
        .output()
        .unwrap();
    let script = out(&o);
    let ran = sh(&script, &repo);
    assert!(
        ran.status.success(),
        "{}\n{}",
        script,
        String::from_utf8_lossy(&ran.stderr)
    );
    let wt = tries.join(format!("{}-wt", chrono_today()));
    assert!(wt.join(".git").is_file(), "worktree .git file missing");
}

#[test]
fn init_snippets_are_valid_shell() {
    let (_root, tries) = sandbox();
    let o = bin()
        .env("SHELL", "/bin/bash")
        .args(["init", tries.to_str().unwrap()])
        .output()
        .unwrap();
    let snippet = out(&o);
    assert!(snippet.starts_with("try() {"));
    let mut check = Command::new("bash")
        .arg("-n")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    check
        .stdin
        .take()
        .unwrap()
        .write_all(snippet.as_bytes())
        .unwrap();
    assert!(check.wait().unwrap().success());

    let fish = bin()
        .env("SHELL", "/usr/bin/fish")
        .arg("init")
        .output()
        .unwrap();
    assert!(out(&fish).starts_with("function try"));
}

#[test]
fn delete_script_removes_only_marked_dirs_and_survives_odd_names() {
    let (root, tries) = sandbox();
    fs::create_dir_all(tries.join("-dash")).unwrap();
    fs::create_dir_all(tries.join("it's quoted")).unwrap();

    for name in ["alpha", "dash", "quoted"] {
        let keys = format!("--and-keys={name},CTRL-D,ENTER,Y,E,S,ENTER");
        let o = try_with(&tries, &[("TRY_DELETE", "remove")], &["exec", &keys]);
        assert_eq!(o.status.code(), Some(0), "{name}");
        let script = out(&o);
        assert!(script.contains("rm -rf"), "{script}");
        let ran = sh(&script, root.path());
        assert!(
            ran.status.success(),
            "{script}\n{}",
            String::from_utf8_lossy(&ran.stderr)
        );
    }
    let left: Vec<String> = fs::read_dir(&tries)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.contains(&"2025-11-15-beta".to_string()));
    assert!(left.contains(&"2025-11-20-gamma".to_string()));
}

#[test]
fn delete_without_typing_yes_does_nothing() {
    let (_root, tries) = sandbox();
    let o = try_cmd(
        &tries,
        &["exec", "--and-keys=alpha,CTRL-D,ENTER,Y,ENTER,ESC"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(tries.join("2025-11-01-alpha").exists());
}

#[cfg(unix)]
#[test]
fn delete_refuses_to_follow_a_symlink_out_of_the_tries_root() {
    let (root, tries) = sandbox();
    let precious = root.path().join("precious");
    fs::create_dir_all(&precious).unwrap();
    std::os::unix::fs::symlink(&precious, tries.join("link")).unwrap();
    let o = try_cmd(
        &tries,
        &["exec", "--and-keys=link,CTRL-D,ENTER,Y,E,S,ENTER,ESC"],
    );
    assert_eq!(o.status.code(), Some(1), "no delete script may be produced");
    assert!(precious.exists());
}

#[test]
fn rename_script_runs() {
    let (root, tries) = sandbox();
    let o = try_cmd(
        &tries,
        &[
            "exec",
            "--and-keys=alpha,CTRL-R,CTRL-U,TYPE=fresh start,ENTER",
        ],
    );
    let script = out(&o);
    assert!(
        script.contains("mv -- '2025-11-01-alpha' 'fresh-start'"),
        "{script}"
    );
    let ran = sh(&script, root.path());
    assert!(ran.status.success());
    assert!(tries.join("fresh-start").is_dir() && !tries.join("2025-11-01-alpha").exists());
}

#[test]
fn graduate_moves_dir_and_leaves_symlink() {
    let (root, tries) = sandbox();
    let projects = root.path().join("projects");
    fs::create_dir_all(&projects).unwrap();
    let o = bin()
        .env("TRY_PROJECTS", &projects)
        .env("NO_COLOR", "1")
        .arg("--path")
        .arg(&tries)
        .args(["exec", "--and-keys=beta,CTRL-G,ENTER"])
        .output()
        .unwrap();
    let script = out(&o);
    assert_eq!(o.status.code(), Some(0), "{script}");
    let ran = sh(&script, root.path());
    assert!(
        ran.status.success(),
        "{script}\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(projects.join("beta").is_dir());
    assert!(fs::symlink_metadata(tries.join("2025-11-15-beta"))
        .unwrap()
        .file_type()
        .is_symlink());
}

fn delete_keys(name: &str) -> String {
    format!("--and-keys={name},CTRL-D,ENTER,Y,E,S,ENTER")
}

fn dirs_in(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(p)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn delete_defaults_to_trash_and_undo_restores_it() {
    let (root, tries) = sandbox();
    fs::write(tries.join("2025-11-01-alpha/note.txt"), "keep me").unwrap();

    let o = try_cmd(&tries, &["exec", &delete_keys("alpha")]);
    let script = out(&o);
    assert_eq!(o.status.code(), Some(0), "{script}");
    assert!(
        !script.contains("rm -rf"),
        "default must not permanently delete: {script}"
    );
    let ran = sh(&script, root.path());
    assert!(
        ran.status.success(),
        "{script}\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(!tries.join("2025-11-01-alpha").exists());
    assert!(
        !dirs_in(&tries).iter().any(|n| n.contains("alpha")),
        "trash must stay out of the list"
    );

    // The trash is visible and the data survived.
    let listing = try_cmd(&tries, &["trash"]);
    assert!(String::from_utf8_lossy(&listing.stderr).contains("2025-11-01-alpha"));

    // Undo: restores, and since it is a single item, cds into it.
    let o = try_cmd(&tries, &["undo"]);
    let script = out(&o);
    assert_eq!(o.status.code(), Some(0), "{script}");
    assert!(
        script.contains(&format!("cd '{}/2025-11-01-alpha'", tries.display())),
        "{script}"
    );
    let ran = sh(&script, root.path());
    assert!(
        ran.status.success(),
        "{script}\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        fs::read_to_string(tries.join("2025-11-01-alpha/note.txt")).unwrap(),
        "keep me"
    );
    assert!(!tries.join(".trash").join("x").exists());

    // Nothing left to undo.
    let o = try_cmd(&tries, &["undo"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Nothing to undo"));
}

#[test]
fn undo_never_overwrites_a_recreated_directory() {
    let (root, tries) = sandbox();
    let script = out(&try_cmd(&tries, &["exec", &delete_keys("alpha")]));
    assert!(sh(&script, root.path()).status.success());
    fs::create_dir_all(tries.join("2025-11-01-alpha")).unwrap();
    fs::write(tries.join("2025-11-01-alpha/new.txt"), "new").unwrap();

    let script = out(&try_cmd(&tries, &["undo"]));
    assert!(sh(&script, root.path()).status.success(), "{script}");
    assert!(tries.join("2025-11-01-alpha/new.txt").exists());
    assert!(tries.join("2025-11-01-alpha-restored").is_dir());
}

#[test]
fn trash_empty_requires_yes_and_honours_age() {
    let (root, tries) = sandbox();
    let script = out(&try_cmd(&tries, &["exec", &delete_keys("alpha")]));
    assert!(sh(&script, root.path()).status.success());

    let refused = try_cmd(&tries, &["trash", "empty"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--yes"));
    assert!(tries.join(".trash").exists());

    // Too fresh for --older-than 30: nothing purged.
    let o = try_cmd(&tries, &["trash", "empty", "--older-than", "30", "--yes"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(tries.join(".trash").exists());

    let o = try_cmd(&tries, &["trash", "empty", "--yes"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(!tries.join(".trash").exists());

    let bad = try_cmd(&tries, &["trash", "empty", "--older-than", "soon", "--yes"]);
    assert_eq!(bad.status.code(), Some(1));
}

fn write_config(dir: &Path, body: &str) {
    fs::create_dir_all(dir.join("try")).unwrap();
    fs::write(dir.join("try/config.toml"), body).unwrap();
}

#[test]
fn config_file_sets_path_and_flag_overrides_it() {
    let (root, tries) = sandbox();
    let xdg = root.path().join("xdg");
    write_config(&xdg, &format!("path = \"{}\"\n", tries.display()));

    // No --path: the config supplies it.
    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .env("NO_COLOR", "1")
        .args(["exec", "--and-keys=bet,ENTER"])
        .output()
        .unwrap();
    assert!(
        out(&o).contains("2025-11-15-beta'"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    // TRY_PATH beats config.
    let other = root.path().join("other");
    fs::create_dir_all(other.join("only-here")).unwrap();
    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .env("TRY_PATH", &other)
        .args(["exec", "--and-keys=ENTER"])
        .output()
        .unwrap();
    assert!(out(&o).contains("only-here'"));

    // `try config` reports the source.
    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .arg("config")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("[config file]") && err.contains("valid"),
        "{err}"
    );
    assert!(o.stdout.is_empty(), "config output must never reach eval");
}

#[test]
fn config_can_select_remove_mode() {
    let (root, tries) = sandbox();
    let xdg = root.path().join("xdg");
    write_config(&xdg, "delete = \"remove\"\n");
    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .env("NO_COLOR", "1")
        .arg("--path")
        .arg(&tries)
        .args(["exec", &delete_keys("alpha")])
        .output()
        .unwrap();
    assert!(out(&o).contains("rm -rf"));
}

#[test]
fn broken_config_blocks_normal_use_but_not_doctor() {
    let (root, tries) = sandbox();
    let xdg = root.path().join("xdg");
    write_config(&xdg, "delete = \"shred\"\n");

    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .arg("--path")
        .arg(&tries)
        .args(["exec", "--and-keys=ENTER"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("config.toml") && err.contains("`delete`"),
        "{err}"
    );

    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .arg("--path")
        .arg(&tries)
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("FAIL  config"));
}

#[test]
fn config_init_writes_a_valid_starter_file_once() {
    let root = tempfile::tempdir().unwrap();
    let xdg = root.path().join("xdg");
    let run = || {
        bin()
            .env("XDG_CONFIG_HOME", &xdg)
            .args(["config", "init"])
            .output()
            .unwrap()
    };
    assert_eq!(run().status.code(), Some(0));
    assert!(xdg.join("try/config.toml").is_file());
    assert_eq!(run().status.code(), Some(1), "must not overwrite");

    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .arg("config")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stderr).contains("(valid)"));
}

#[test]
fn doctor_reports_on_stderr_and_exit_code_reflects_failures() {
    let (root, tries) = sandbox();
    let o = try_cmd(&tries, &["doctor"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.stdout.is_empty(), "doctor output must never reach eval");
    assert!(
        err.contains("ok    tries") && err.contains("3 dirs"),
        "{err}"
    );
    assert!(err.contains("trash mode; trash is empty"), "{err}");
    assert_eq!(o.status.code(), Some(0), "{err}");

    // tries path is a regular file: unusable.
    let file = root.path().join("afile");
    fs::write(&file, "").unwrap();
    let o = try_cmd(&file, &["doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("FAIL  tries"));
}

#[test]
fn bad_try_delete_value_is_rejected() {
    let (_root, tries) = sandbox();
    let o = try_with(
        &tries,
        &[("TRY_DELETE", "shred")],
        &["exec", "--and-keys=ENTER"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("TRY_DELETE"));
}

fn git_in(dir: &Path, args: &[&str]) {
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

/// tries/{clean,dirty,ahead} are repos; tries/plain is not.
fn git_sandbox() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let tries = root.path().join("tries");
    for n in ["clean", "dirty", "plain"] {
        fs::create_dir_all(tries.join(n)).unwrap();
    }
    for n in ["clean", "dirty"] {
        git_in(&tries.join(n), &["init", "-q"]);
        git_in(
            &tries.join(n),
            &["commit", "-q", "--allow-empty", "-m", "init"],
        );
    }
    fs::write(tries.join("dirty/scratch.txt"), "x").unwrap();

    let bare = root.path().join("origin.git");
    fs::create_dir_all(&bare).unwrap();
    git_in(&bare, &["init", "-q", "--bare"]);
    git_in(&tries, &["clone", "-q", bare.to_str().unwrap(), "ahead"]);
    git_in(
        &tries.join("ahead"),
        &["commit", "-q", "--allow-empty", "-m", "one"],
    );
    git_in(
        &tries.join("ahead"),
        &["push", "-q", "-u", "origin", "HEAD"],
    );
    git_in(
        &tries.join("ahead"),
        &["commit", "-q", "--allow-empty", "-m", "two"],
    );
    (root, tries)
}

fn frame_line(frame: &str, name: &str) -> String {
    frame
        .lines()
        .find(|l| l.contains(name))
        .unwrap_or_else(|| panic!("no row for {name} in:\n{frame}"))
        .to_string()
}

#[test]
fn list_shows_git_markers() {
    let (_root, tries) = git_sandbox();
    let o = try_cmd(&tries, &["exec", "--and-exit"]);
    let frame = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(frame_line(&frame, "clean").contains('✓'), "{frame}");
    let dirty = frame_line(&frame, "dirty");
    assert!(dirty.contains('●') && !dirty.contains('✓'), "{dirty}");
    let ahead = frame_line(&frame, "ahead");
    assert!(ahead.contains("↑1") && !ahead.contains('✓'), "{ahead}");
    let plain = frame_line(&frame, "plain");
    assert!(
        !plain.contains('✓') && !plain.contains('●') && !plain.contains('?'),
        "{plain}"
    );
}

#[test]
fn git_markers_can_be_disabled_by_env_and_config() {
    let (root, tries) = git_sandbox();
    let o = try_with(&tries, &[("TRY_GIT_STATUS", "0")], &["exec", "--and-exit"]);
    let frame = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(!frame.contains('✓') && !frame.contains('●'), "{frame}");

    let xdg = root.path().join("xdg");
    write_config(&xdg, "git_status = false\n");
    let o = bin()
        .env("XDG_CONFIG_HOME", &xdg)
        .env("NO_COLOR", "1")
        .arg("--path")
        .arg(&tries)
        .args(["exec", "--and-exit"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&o.stderr).contains('●'));
}

#[test]
fn delete_dialog_warns_about_unsaved_git_work() {
    let (_root, tries) = git_sandbox();
    let render = |env: &[(&str, &str)], name: &str| {
        let keys = format!("--and-keys={name},CTRL-D,ENTER");
        let o = try_with(&tries, env, &["exec", "--and-exit", &keys]);
        String::from_utf8_lossy(&o.stderr).into_owned()
    };

    let dirty = render(&[], "dirty");
    assert!(dirty.contains("⚠ dirty: uncommitted changes"), "{dirty}");

    let ahead = render(&[], "ahead");
    assert!(ahead.contains("⚠ ahead: 1 unpushed commit(s)"), "{ahead}");

    // A clean repo with nothing to lose gets no warning in trash mode...
    let clean = render(&[], "clean");
    assert!(!clean.contains('⚠'), "{clean}");
    // ...but a permanent delete flags commits that exist only locally.
    let clean_rm = render(&[("TRY_DELETE", "remove")], "clean");
    assert!(clean_rm.contains("⚠ clean: no upstream"), "{clean_rm}");

    // Not a repo: nothing to say.
    assert!(!render(&[("TRY_DELETE", "remove")], "plain").contains('⚠'));

    // Disabled: no probing, no warning.
    let off = render(&[("TRY_GIT_STATUS", "0")], "dirty");
    assert!(!off.contains('⚠'), "{off}");
}

#[test]
fn doctor_and_config_mention_git_status() {
    let (_root, tries) = sandbox();
    let o = try_cmd(&tries, &["doctor"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("list markers on"));
    let o = try_with(&tries, &[("TRY_GIT_STATUS", "off")], &["config"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("git       off  [$TRY_GIT_STATUS]"));
    let o = try_with(&tries, &[("TRY_GIT_STATUS", "maybe")], &["config"]);
    assert_eq!(o.status.code(), Some(1));
}
