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

Rules live in your config, so which tools get wrapped is yours to choose:

```toml
use = ["devenv", "git", "wt"]

[[rule]]
id    = "devenv-launch"
tool  = ["nvim"]
phase = "pre"
when  = { root_has = "devenv.nix", env_unset = "DEVENV_ROOT" }
do    = { wrap = ["devenv", "shell", "--no-tui", "--"] }
```

(`examples/config.toml` is this file.)

- **Phases:** `pre`, `post`, `notfound`, `chpwd`, `prompt`, `session_start`.
- **Matching:** `tool`, `sub` (the first subcommand is one of these) and
  `args_start` (the positional arguments start with these). Global flags such
  as `git -C path` are skipped.
- **Conditions** (all must hold): `root_has`, `env_set`, `env_unset`,
  `command_missing`, `agent` (true or false), and `git = "root_off_default"`.
  A fixed set of named checks, no scripting, so a rules file is safe to read.
  Git state is read from `.git` directly; no `git` process runs.
- **Actions:** `hint`, `warn`, `block`, `wrap`, `set_env`. Messages can use
  `{branch}`, `{default}` and `{root}`.
- **Agents:** a caller is an agent when `AGENT` or `CLAUDECODE` is set in its
  environment. Set `AGENT=1` for anything else.

See `docs/protocol.md` for the event and decision JSON that handlers speak.

## Agents

Agent shells often skip your rc files, so the shell glue never runs there.
`trapline agent claude` answers Claude Code's hook calls with the same rules:

```json
{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command", "command": "trapline agent claude", "timeout": 10}]}],
  "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "trapline agent claude", "timeout": 10}]}]
}}
```

- **`SessionStart`** becomes a `session_start` event; messages go to the agent as
  context (and to you as a notice).
- **`PreToolUse` on `Bash`** is split into its simple commands (`cd x && git
  commit` is two) and each becomes a `pre` event. A `block` denies the call and
  the reason is shown to the agent; a `hint` or `warn` is added as context and
  the normal permission flow continues.
- Every event carries `AGENT=1`, so `agent = true` rules apply.
- Use a separate config for agents so shell wrappers and agent rules stay
  apart: `TRAPLINE_CONFIG=$HOME/.config/trapline/agent.toml trapline agent claude`.
- Limits: the parser does not look inside `$(...)`, subshells or `cd`, so a rule
  can miss a command hidden there. Treat it as guidance, not a sandbox.

## Plugins

Three extension points, none of which need a change to trapline:

- **Packs.** Drop `mypack.toml` in `~/.config/trapline/packs/` and `use =
  ["mypack"]`. A user pack shadows a built-in of the same name.
- **Handlers.** Any program that reads an Event as JSON on stdin and prints a
  Decision as JSON (any subset of fields) on stdout:

  ```toml
  [[handler]]
  id      = "my-check"
  command = ["my-check", "--json"]
  phase   = ["pre"]
  tool    = ["kubectl"]
  timeout_ms = 2000   # killed after this; a failing handler warns, never blocks
  ```

- **Agent adapters.** `trapline agent foo` runs `trapline-agent-foo` from `PATH`
  (like `git foo`). The adapter translates its agent's hook payload to an Event,
  calls `trapline eval` (Event JSON on stdin, Decision JSON on stdout), and
  translates back. Write it in any language.

## Packs

`nudge packs` lists them and `nudge show <pack>` prints one.

| Pack | What it does |
|---|---|
| `devenv` | Prompt markers (`⬡ devenv` inside, `⬡ off` in a devenv repo you have not entered); a not-found hint pointing at `dev`. |
| `git` | In a root checkout off the default branch, `commit`/`push`/`add`/`merge`/`rebase` warn (firmer wording for an agent) and never block. Raw `git worktree add` warns a human and blocks an agent, and points at `wt`. |
| `devenv-agent` | Agent-only: at session start, tells the agent the repo has a devenv and how to use it. Quiet in repos without a `devenv.nix`. |
| `wt` | `wt switch` warns when `devenv` is not on `PATH` (the post-switch warm-up would be skipped). `wt remove` reminds an agent to check unpushed work. |

## Status

Early. Working: the event protocol, the rules engine, the `devenv`, `git` and
`wt` packs, zsh glue. Planned: `once` for hints, bash and fish glue,
`nudge explain`, repo-level `.trapline.toml` with an allow step (repo rules
limited to `hint`/`warn` until allowed), `trapline serve` (JSON-RPC) and an
editor plugin.

## License

MIT or Apache-2.0, at your option.
