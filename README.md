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
