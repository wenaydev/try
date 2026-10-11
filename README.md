# try

Fresh directories for every vibe. A Rust port of [tobi/try](https://github.com/tobi/try):
fuzzy-find, create, clone and clean up throwaway project directories.

```bash
cargo install --path .          # installs the `try` binary

# bash / zsh
eval "$(try init ~/src/tries)"  # or: try install
# fish
try init ~/src/tries | source
```

### Always run the latest build (development machines)

Point a symlink on your `PATH` at the release binary and let the shell integration re-initialise
on every new shell:

```bash
ln -sfn "$PWD/target/release/try" ~/.local/bin/try          # ~/.local/bin must be on PATH
mkdir -p ~/.bashrc.d && cat > ~/.bashrc.d/try.sh <<'EOF'
if try_bin=$(type -P try); then eval "$("$try_bin" init)"; fi
unset try_bin
EOF
```

(`~/.bashrc.d/` is sourced by Fedora's stock `~/.bashrc`; elsewhere put the `if` block in `~/.bashrc`.
zsh: `~/.zshrc`. fish: `~/.config/fish/conf.d/try.fish` with `try init | source`.)

After that, `cargo build --release` is the whole deployment: the wrapper function calls the binary
by path on every use, so even shells that are already open pick the new build up on the next `try`,
and new shells regenerate the wrapper itself from it. `try doctor` confirms the wiring.
A debug build (`cargo build`) does not affect it, and `cargo install` writes to `~/.cargo/bin`,
a different location.

## Usage

```
try                              browse everything (fuzzy filter, recent first)
try redis                        start with "redis" typed; Enter on "Create new" makes 2025-08-17-redis
try clone <url> [name]           clone into 2025-08-17-user-repo (also: try <url>)
try https://github.com/u/r/pull/9   clone and check out the PR, detached
try . <name>                     worktree of the current repo (plain dir outside git)
try ./path/to/repo [name]        worktree of another repo
try worktree [dir] [name]        explicit form
```

Keys: `↑/↓` `Ctrl-P/N` `Ctrl-J/K` navigate · `Enter` select · `Ctrl-T` new ·
`Ctrl-R` rename · `Ctrl-G` graduate to a real project · `Ctrl-D` mark for delete
(Enter, then type `YES`) · `Esc` cancel.

## Git markers

Directories that are git repositories (clones and worktrees) get a marker next to the age:

| marker | meaning |
|---|---|
| `●` | uncommitted changes (modified, staged or untracked files) |
| `↑2` | 2 commits not pushed to the upstream branch |
| `↓1` | 1 commit behind upstream |
| `✓` | clean and in sync (also shown for repos with no upstream) |
| `?` | git could not answer (timed out or refused, e.g. "dubious ownership") |

Only the rows on screen are checked, in the background, so the list is usable immediately and
markers fill in a moment later. Each check is one `git status` that is cut short at the first
changed file and killed after 3 s, and it never takes git's optional locks.

The delete confirmation also warns about unsaved work (`⚠ name: uncommitted changes`,
`⚠ name: 2 unpushed commit(s)`, and in `remove` mode `no upstream, commits may exist only here`).
Turn it all off with `git_status = false` or `TRY_GIT_STATUS=0`.

## Deleting is recoverable

`Ctrl-D` (then `Enter`, then type `YES`) moves directories to `<tries>/.trash/<batch>/`
instead of running `rm -rf`. The trash is on the same filesystem (an atomic rename) and
hidden from the list.

```
try undo                               # restore the last batch (never overwrites; adds -restored)
try trash                              # list what is in the trash
try trash empty --older-than 30 --yes  # purge for real (without --yes it only refuses)
```

Prefer the old behavior? `delete = "remove"` in the config, or `TRY_DELETE=remove`.

## Configuration

Precedence: flag > environment > `~/.config/try/config.toml` > default.
`try config` shows the effective values and where each came from; `try config init`
writes a commented starter file.

| config key | env var | default |
|---|---|---|
| `path` | `TRY_PATH` (or `--path`) | `~/src/tries` |
| `projects` | `TRY_PROJECTS` | parent of `path` |
| `delete` = `"trash"`/`"remove"` | `TRY_DELETE` | `trash` |
| `colors` = `true`/`false` | `NO_COLOR` | `true` |
| `git_status` = `true`/`false` | `TRY_GIT_STATUS` | `true` |

A malformed config is an error (not silently ignored); `try doctor` still runs so you can see why.

## Troubleshooting

`try doctor` checks the binary, config, tries directory (exists and writable), shell
wrapper in your rc files, `/dev/tty`, `git`, the graduate destination and the trash.
Exit code 1 means something failed. Its output goes to stderr only, like `config`,
`trash` and `--help`, because stdout is `eval`ed by the wrapper.

## How it works

A process cannot `cd` its parent shell. `try init` prints a small function that
runs `try exec …` (UI on stderr, script on stdout) and `eval`s the script.
Everything the script touches is single-quoted.

## Development

```bash
cargo test                                   # unit + black-box tests
./target/release/try --path DIR --and-keys='bet,ENTER' exec   # scripted keys, no TTY needed
```

`--and-keys` accepts symbolic tokens (`DOWN,CTRL-D,TYPE=YES,ENTER`) or raw bytes;
`--and-exit` renders a single frame; `TRY_WIDTH`/`TRY_HEIGHT` fake the terminal size.
The upstream `spec/tests/runner.sh` suite runs unmodified against this binary (399/399 with
`TRY_DELETE=remove`; with the default trash mode three delete assertions that look for `rm -rf`
fail, by design). Point `XDG_CONFIG_HOME` somewhere empty when running it.

## License

MIT. Based on [tobi/try](https://github.com/tobi/try) (c) 2025 Tobi Lutke; see `LICENSE`.
