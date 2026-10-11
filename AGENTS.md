# AGENTS.md

Guidance for AI agents (and humans) working in this repo.

## What this is

`try` is a Rust port of [tobi/try](https://github.com/tobi/try): a fuzzy directory
selector for throwaway experiment directories (`~/src/tries/2025-08-17-redis-pool`), plus
clone / PR checkout / worktree / rename / graduate / delete. Beyond the port it adds a config
file, a recoverable trash (`try undo`), `try doctor`, and git status markers.
Binary name is `try`, crate is `try-cli` (lib `try_cli`). MIT, MSRV 1.82, edition 2021.

## The one design rule: stdout is a script

A child process cannot `cd` its parent. The shell wrapper from `try init` runs
`try exec ...`, captures **stdout**, and `eval`s it when the exit code is 0 (otherwise `echo`s it).
The interactive UI is drawn on **stderr** (the wrapper sends it to `/dev/tty`).

Consequences you must preserve:

- On success, stdout is **only** a script (see `script.rs`); exit 0.
- Cancel = `Cancelled.` on stdout, exit 1. Bare `try` with no args = usage on stderr, exit 2.
- Errors, help, version, `doctor`, `config`, `trash`, `undo` messaging go to **stderr**. Anything
  else on stdout with exit 0 gets `eval`ed by the user's shell. Commands that only report
  return `Reply::Exit(code)` with empty stdout.
- Scripts must run under bash, zsh **and fish**: `&&` chains, single-quoted paths via
  `script::quote`, no brace groups, no `if/then` at top level (wrap in `/usr/bin/env sh -c`, see
  `script::worktree`). Never interpolate unquoted user input (upstream's raw `git clone '{uri}'` was an injection bug).

## Layout

| Path | Role |
|---|---|
| `src/cli.rs` | arg parsing (options may appear anywhere), command dispatch, `Reply` |
| `src/config.rs` | `config.toml`, `Settings::resolve` (flag > env > config > default) |
| `src/script.rs` | all shell-script generation, quoting, terminal-tab rename hooks |
| `src/tui/selector.rs` | state machine **and** rendering, no terminal I/O |
| `src/tui/term.rs` | raw mode, frame output, event loop, scripted (`--and-keys`) runner |
| `src/tui/{keys,input,style}.rs` | key model + `--and-keys` parser, text field, ANSI/width helpers |
| `src/fuzzy.rs` | scoring and highlight positions (ported formula) |
| `src/entries.rs` | reads the tries dir, recency base score |
| `src/gitstatus.rs` | bounded async `git status` probes for list markers |
| `src/trash.rs` | `<tries>/.trash/<batch>/` layout, restore planning, purge |
| `src/doctor.rs`, `src/init.rs`, `src/git.rs`, `src/naming.rs`, `src/paths.rs` | as named |
| `tests/cli.rs` | black-box tests of the real binary, some execute emitted scripts in `sh` |

## Commands

`cargo` lives in `~/.cargo/bin` and may not be on `PATH` (use `~/.cargo/bin/cargo`).

```bash
cargo test                      # unit (in-module) + tests/cli.rs
cargo clippy --all-targets      # keep at zero warnings
cargo fmt                       # run before committing
cargo build --release
# scripted, no TTY needed (UI frame goes to stderr, script to stdout):
./target/release/try --path DIR --and-keys='bet,ENTER' exec
./target/release/try --path DIR --and-exit exec          # one frame, exit 1
```

`--and-keys` takes tokens (`DOWN,CTRL-D,TYPE=YES,ENTER`; upper-case names are keys, other text is
typed) or raw bytes. `TRY_WIDTH` / `TRY_HEIGHT` fake the terminal size. A scripted run must end in
a terminating key or it is treated as cancel.

### Upstream conformance suite

The upstream repo ships a language-agnostic suite. It is not vendored; clone it when needed:

```bash
git clone --depth 1 https://github.com/tobi/try /tmp/try-ref && cd /tmp/try-ref
export XDG_CONFIG_HOME=/nonexistent        # keep your real config out of the run
TRY_DELETE=remove bash spec/tests/runner.sh /path/to/target/release/try     # expect 399/399
TRY_DELETE=remove bash spec/tests/tmux/run.sh /path/to/target/release/try   # expect 37/37 (needs tmux)
```

Without `TRY_DELETE=remove`, exactly three assertions fail (they grep for `rm -rf`); that is by
design since trash mode is the default.

## Testing conventions

- `tests/cli.rs::bin()` isolates the binary: clears `TRY_*`/`HERDR_*`/`CMUX_*` and points
  `XDG_CONFIG_HOME` at a nonexistent dir. Use it (or `try_with`) for every spawn, never raw `Command`.
- Prefer testing behavior by executing the emitted script in `sh` against a tempdir over
  asserting on script text.
- Selector tests drive `handle_key` / `render` directly; no terminal needed.
- When driving the real TUI in tmux, **do not redirect stderr** (`2>/dev/null` makes the UI refuse
  to start: it needs a TTY on stderr). Redirect stdout only.
- Git tests need `git` on PATH and pass `-c user.email -c user.name` themselves.

## Behavior decisions worth knowing

Deliberate deviations from upstream (don't "fix" them back):

- **Ctrl-K / Ctrl-J navigate** (README says so; upstream code actually kills-to-end on Ctrl-K).
  Ctrl-K still kills to end of line inside rename/graduate dialogs.
- **Delete defaults to trash** (`.trash` inside the tries root, hidden from the list). `delete = "remove"`
  / `TRY_DELETE=remove` restores `rm -rf`. The remove-mode script matches the upstream spec
  (`test -d 'x' && rm -rf 'x'`); names starting with `-` get a `./` prefix.
- `try worktree <name>` means the **current repo** (upstream treated it as a repo path). A first arg
  is a repo only if it is `dir`, `.`, `..`, starts with `~`, or contains `/`.
- Metadata (age, score) is hidden rather than partially shown when a long name leaves no room.
- No PowerShell wrapper: the emitted script is POSIX, so it cannot work there.
- Selecting an already-graduated symlink for graduate is refused; deleting anything that resolves
  outside the tries root is refused (symlink-escape protection in `Selector::validate_delete`).
- Ties in fuzzy score keep name order (entries are sorted by name on load) for determinism.

Reserved first arguments (use `try cd <word>` to search for a directory named like one):
`exec clone init install worktree undo trash doctor config cd`, plus anything starting with `.`
(`try .` / `try ./path`) and anything that looks like a git URL.

Config precedence is always flag > env > `config.toml` > default. A malformed config blocks every
command except `doctor` and `config`.

Icons: avoid emoji that need a variation selector (e.g. ✏️); terminals disagree on their width and
the layout math (`unicode-width`) goes wrong. Rename uses 📝 for that reason.

## Adding a command

1. Add the arm in `cli::dispatch` and a `cmd_*` fn returning a `Reply`.
2. Anything that is not a script goes to stderr and returns `Reply::Exit`.
3. Add it to `HELP`, the README, and the reserved-words list above.
4. Add a black-box test in `tests/cli.rs`; if it emits a script, run the script in `sh`.

## Git / remote

`origin` is `git@github.com:wenaydev/try.git`, default branch `main`. Pushing needs an SSH key and
GitHub's host key on the machine; if `git ls-remote origin` says `Host key verification failed`, the
account has no `~/.ssh/known_hosts` entry yet. Do not disable host key checking to work around it.
Commit messages: imperative subject, body explains why. Do not commit `target/`.

## Ideas not done yet

`try list --json`, preview pane, PageUp/PageDown + mouse in the list, `nucleo`-style matcher,
shell completions / manpage, release workflow, `--dry-run` for exec mode.
