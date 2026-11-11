# trapline

Guidance at the moment of action, from one rule set shared by your shell,
your editor, and your agents.

trapline sits at the command boundary. It turns shell events (a prompt about to
render, a `cd`, a command about to run, a command that was not found) into
structured events, and asks a handler what to do. The built-in handler is
**nudge**: declarative rules that start by informing (`hint`, `warn`) and can
escalate to `block` or `wrap`.

```
shell glue ──event──▶ trapline ──▶ handler (nudge) ──decision──▶ shell glue
agent hooks ─────────▶ trapline          editors ───────────────▶ trapline
```

- **`trapline`** (alias **`tl`**): the runtime. Emits shell glue, evaluates events.
- **`nudge`**: the built-in rules engine and its packs.

## Goal and non-goals

The goal is the right nudge, for the right caller, at the right time. Defaults
inform; blocking and wrapping are opt-in.

Not goals: a general hook framework (git hooks, `wt` hooks and your agent's own
hooks already exist), an environment manager (use devenv, mise, nix), or a task
runner. Where a tool already owns a job, a pack checks state and points at it.

## Quickstart

```sh
cargo install --path crates/trapline --path crates/nudge
```

`~/.config/trapline/config.toml`:

```toml
use = ["devenv"]
```

`.zshrc`:

```sh
eval "$(trapline hook zsh)"
```

Check one event by hand:

```sh
trapline check pre nvim --format json
```

## Rules

```toml
[[rule]]
id    = "devenv-launch"
tool  = ["nvim"]
phase = "pre"
when  = { root_has = "devenv.nix", env_unset = "DEVENV_ROOT" }
do    = { wrap = ["devenv", "shell", "--no-tui", "--"] }
```

- **Phases:** `pre`, `post`, `notfound`, `chpwd`, `prompt`.
- **Conditions** (all must hold): `root_has`, `env_set`, `env_unset`. A fixed
  set of named checks, no scripting, so a rules file is safe to read.
- **Actions:** `hint`, `warn`, `block`, `wrap`, `set_env`.

See `docs/protocol.md` for the event and decision JSON that handlers speak.

## Status

Early. Working: the event protocol, the rules engine, the `devenv` pack, zsh
glue. Planned: `git` and `wt` packs, `once` for hints, bash and fish glue,
`nudge explain`, repo-level `.trapline.toml` with an allow step (repo rules
limited to `hint`/`warn` until allowed), `trapline serve` (JSON-RPC) and an
editor plugin.

## License

MIT or Apache-2.0, at your option.
