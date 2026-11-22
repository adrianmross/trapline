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

Start with [first use, team guidance and quiet hints](docs/first-use.md).
On macOS, `brew install adrianmross/private/trapline` installs both commands.
The source-build alternative is below.


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

- **Phases:** `pre`, `post`, `notfound`, `chpwd`, `prompt`, `session_start`, `sandbox`,
  and `tool` (an agent's own tool call, such as Claude Code's `Agent`; its input
  fields arrive as `key=value` arguments; see `docs/protocol.md`).
- **Matching:** `tool`, `sub` (the first subcommand is one of these) and
  `args_start` (the positional arguments start with these). Global flags such
  as `git -C path` are skipped. `args_has` matches any raw argument, flags
  included (`--cdp` also matches `--cdp=9222`), for tools whose global flags take
  values, where `sub` can be fooled by a flag's value. `args_lacks` is its
  opposite: no argument is one of these.
- **Conditions** (all must hold): `root_has`, `env_set`, `env_unset`,
  `command_missing`, `path_missing` (`~/` is your home), `agent` (true or false), and `git = "root_off_default" | "in_repo" | "linked_worktree"`.
  A fixed set of named checks, no scripting, so a rules file is safe to read.
  Git state is read from `.git` directly; no `git` process runs.
- **Actions:** `hint`, `warn`, `block`, `wrap`, `set_env`, and for the sandbox
  phase `deny_write`, `allow_write`, `lock_write`. Messages and paths can use
  `{branch}`, `{default}`, `{root}` and `{main_root}`.
- **`run`** asks a tool for its own report: `do = { run = [argv], each =
  "...", timeout_ms = 5000 }` runs the argv in the event's cwd and adds one
  warning per item of the JSON array it prints. In `each`, `{field}` is the
  item's field and a `[...]` part is dropped unless all its fields are present.
  A missing program, a non-zero exit, a timeout or other output adds nothing.
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
- **`PreToolUse` on any other tool** becomes one `tool` event named after the
  tool, its input as `key=value` arguments. Add a `PreToolUse` entry with no
  matcher (or `"Agent|EnterWorktree"`) to send those calls too.
- **`PreToolUse` on `Bash`** is split into its simple commands (`cd x && git
  commit` is two) and each becomes a `pre` event. A `block` denies the call and
  the reason is shown to the agent; a `hint` or `warn` is added as context and
  the normal permission flow continues.
- Every event carries `AGENT=1`, so `agent = true` rules apply.
- Use a separate config for agents so shell wrappers and agent rules stay
  apart: `TRAPLINE_CONFIG=$HOME/.config/trapline/agent.toml trapline agent claude`.
- Limits: the parser does not look inside `$(...)`, subshells or `cd`, so a rule
  can miss a command hidden there. Treat it as guidance, not a sandbox.

## Codex

Codex hooks send the same payload and read the same response as Claude Code
(`hookSpecificOutput` with `additionalContext`, or `permissionDecision: "deny"`
and `permissionDecisionReason`), so `trapline agent codex` and `trapline agent
claude` are one implementation. It handles `SessionStart` and `PreToolUse`,
including canonical `Bash` and unified-exec `cmd` payloads; other tools become
`tool` events, as for Claude Code. It recognizes
`rtk git ...` and `rtk proxy git ...` without changing the command. Explicit
unified-exec workdirs are used when evaluating the command's repository context.
Unsupported tools and malformed events produce no decision.

Start with `examples/codex.toml` and merge the two hook groups from
`examples/codex-hooks.json` into your existing Codex hook configuration. Keep
existing hooks. Use an absolute installed trapline path if it is not on the
hook process's PATH. Review the exact definitions using Codex `/hooks`; the
adapter does not modify hook-trust records or bypass review. Newly configured
hooks affect new sessions; do not restart active work to pick them up.

The sample profile gives advice only. The adapter can return an explicit denial
when a selected rule blocks, but never grants approval or rewrites a tool call.
Wrapper rules are surfaced as advice to enter the environment explicitly.
`PermissionRequest` is not handled. Existing Codex sandbox and approval controls
remain in force; do not stack trapline's OS sandbox around Codex by default.
The built-in devenv guidance distinguishes an unset `DEVENV_ROOT` from a set one
and asks you to confirm it matches the worktree after switching.

Codex hooks must use a supported local execution path. Cloud-orchestrated
sessions may not load local command hooks; skipped or untrusted hooks do not
prove coverage. Test with a real disposable Codex session after review.

For this repository, `devenv tasks run project:doctor`, `project:test`, and
`project:verify` provide the environment/readiness, test, and format/lint steps.
They are explicit tasks, not shell-entry deployment actions. Keep cloud target
validation in the owning CLI and CA installation in the reviewed trust tool.
devenv manages project dependencies and services; Rex keeps remote terminals,
and the handoff queue supplies approved laptop-only inputs.

## Fast prompts

`trapline hook zsh` compiles the `prompt`, `chpwd` and `notfound` rules to plain
zsh when it can, so a prompt starts no process. A phase is compiled only if
every rule in it is expressible in zsh (no `git` conditions, no `{branch}`
templates) and no external handler covers it; otherwise that phase keeps
calling the binary. `--no-compile` forces the binary everywhere. A test checks
the compiled output equals the binary's in each scenario.

## Sandbox

The same rules can produce an enforced boundary. In the `sandbox` phase a rule
returns `deny_write`, `allow_write` and `lock_write` paths (a later tier wins
over an earlier one), `deny_read` paths (`~/` expands to your home directory),
and `allow_domains`. The `git` pack makes a repo's own checkout read-only,
keeps its `.git` writable so worktrees can commit, locks the checkout's own
`HEAD` and `index` so its branch cannot be switched, and keeps the current
worktree writable.

```sh
trapline run --profile agent -- claude                      # rules + sandbox, then exec
sandbox-exec -p "$(trapline sandbox seatbelt)" claude      # macOS, by hand
trapline sandbox bwrap                                      # Linux: argv, one per line
trapline sandbox claude-settings                            # JSON for Claude Code's `sandbox` setting
trapline sandbox runtime > srt.json                         # for `srt --settings srt.json -- cmd`
```

`run` points `TRAPLINE_CONFIG` at `<config dir>/<profile>.toml`, wraps the command
in `sandbox-exec` (macOS) or `bwrap` (Linux), and execs it. `--no-sandbox` sets
only the profile.

One rule set, four targets. What each can express:

| | Seatbelt | bwrap | Claude Code `sandbox` | sandbox-runtime |
|---|---|---|---|---|
| write allow / deny | yes | yes | `filesystem.allowWrite` / `denyWrite` | `filesystem.allowWrite` / `denyWrite` |
| `lock_write` (root's `HEAD`, `index`) | yes | yes | yes, as `denyWrite` | yes, as `denyWrite` |
| deny inside a deny, carve-out (`.git` inside the denied root) | yes | yes | no: a deny beats an allow | no: a deny beats an allow |
| `deny_read` | yes | yes (dir: tmpfs, file: `/dev/null`) | `filesystem.denyRead` | `filesystem.denyRead` |
| `allow_domains` | no, a comment says so | no, ignored | `network.allowedDomains` | `network.allowedDomains` |

The two JSON targets are allow-list models where a deny wins over an allow, so
the generator drops a deny that holds an allowed path (the root checkout holds
its `.git`) and relies on the allow-list leaving it read-only. Run from a linked
worktree that gives the same protection as Seatbelt: only the worktree and the
root's `.git` are writable, and `HEAD` and `index` are locked. Run from the root
checkout itself nothing allows the cwd, so the deny is kept and `.git` goes
read-only too. Claude Code always allows its cwd for writes, so there the deny is
what protects the root. `claude-settings` omits `network` when no rule names a
domain (Claude Code prompts for unlisted hosts); `runtime` always lists
`allowedDomains`, empty if none, because the runtime denies all network unless
hosts are listed. `runtime` also adds the cwd to `allowWrite` (it allows no writes
otherwise), except when the cwd is denied.

Field names were checked against the Claude Code sandboxing docs and the
sandbox-runtime README: `sandbox.enabled`, `sandbox.filesystem.{allowWrite,denyWrite,denyRead}`,
`sandbox.network.allowedDomains`, and for the runtime `filesystem.{allowWrite,denyWrite,denyRead}`
and `network.allowedDomains`. Not emitted: `allowRead`, `deniedDomains`, Claude
Code's `credentials`, and anything under `excludedCommands`. A path that does not
exist yet gets no protection on Linux in either JSON target, so `.git/index` of a
fresh repo is not locked until it exists.

Verified on macOS, including `claude -p` run through `trapline run` and a
`deny_read` file that `cat` cannot open: writes in a linked worktree and commits
from it work; writes to the root checkout and a branch switch there are denied.
The JSON targets were checked for shape and content only, not run through Claude
Code or `srt`. `bwrap` output is generated but untested on Linux (including the
`--tmpfs` and `/dev/null` read hiding). `sandbox-exec` cannot nest in another
sandbox, so under Claude Code's own sandbox use `claude-settings` instead.

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
| `git` | In a root checkout off the default branch, `commit`/`push`/`add`/`merge`/`rebase` warn (firmer wording for an agent) and never block. Raw `git worktree add` warns a human and blocks an agent, and points at `wt`. Claude Code's `Agent` with `isolation: "worktree"` is blocked, and so is `EnterWorktree name=` without `path=` when the worktrunk plugin (`~/.claude/plugins/cache/worktrunk`) is not installed. |
| `devenv-agent` | Agent-only: at session start, tells the agent the repo has a devenv and how to use it. Quiet in repos without a `devenv.nix`. |
| `agent-browser` | Agent-only, for [agent-browser](https://agent-browser.dev): blocks the `eval`-category actions (page scripts can undo its domain allowlist), and `--cdp`, `--auto-connect`, `--profile`, `--restore`, `--state` and `state load` (they attach to an existing browser, so `--allowed-domains` cannot apply). Each block says the command to use instead. |
| `agent-browser-strict` | Opt-in, with `agent-browser`: blocks an agent's `open` until `AGENT_BROWSER_ALLOWED_DOMAINS` is set in its environment. |
| `wt` | `wt switch` warns when `devenv` is not on `PATH` (the post-switch warm-up would be skipped). `wt remove` reminds an agent to check unpushed work. At session start, lists `wt step doctor` findings, one line each with its fix (needs a `wt` that has `step doctor --format=json`). |

### Allowed domains for `agent-browser`
The packs do not know your domains; agent-browser enforces its allowlist
(`--allowed-domains`, `AGENT_BROWSER_ALLOWED_DOMAINS`, or `allowedDomains` in
`~/.agent-browser/config.json`, per its security docs). Set it where the agent
starts. To have trapline require it, add `agent-browser-strict` to `use`, or
write your own rule with the existing keys, for example:

```toml
[[rule]]
id = "browser-no-upload"
tool = ["agent-browser"]
args_has = ["upload"]
phase = "pre"
when = { agent = true }
do = { warn = "uploads are blocked for agents; ask the user to do it", block = true }
```

Not covered: whether an `--allowed-domains` flag is present on the same command
(rules cannot test for a missing flag), and agent-browser run through a wrapper
such as `npx`.

## Status

Early. Working: the event protocol, the rules engine, the `devenv`, `git` and
`wt` packs, zsh glue. Planned: bash and fish glue,
`nudge explain`, repo-level `.trapline.toml` with an allow step (repo rules
limited to `hint`/`warn` until allowed), `trapline serve` (JSON-RPC) and an
editor plugin.

## License

MIT or Apache-2.0, at your option.

## Explaining and logging
`trapline explain <phase> [tool]` lists each rule in that phase and why it did or
did not apply to the current directory and environment. Rules that fire through
the binary or an agent hook are appended to `$XDG_STATE_HOME/trapline/fired.jsonl`
(`~/.local/state` by default); read it with `trapline log [-n 20]` or
`trapline log --stats`. Rules compiled into the shell glue spawn nothing, so they
are not logged.

## Cost
The release binary is under 1 MB (LTO, stripped, size-optimized). In zsh the
prompt, `chpwd`, not-found, and `pre` phases compile to plain functions when
every rule in them can be expressed in zsh, including reading branch state from
`.git` with builtins: a wrapped `git` call costs ~0.1 ms instead of ~7 ms. The
`post` phase costs nothing unless a rule uses it. A phase with an external
handler, or a rule zsh cannot express, keeps calling the binary
(`trapline hook zsh --no-compile` forces that everywhere).

## Shared project checks

Run `devenv tasks run project:doctor project:test project:verify` locally or on
Linux. Listing all three supports older devenv clients that only run named
tasks. The lock pins the project toolchain; doctor reports the actual selected
environment. Tests cover adapter decisions, RTK commands and additive hook
installation. Verify also checks formatting and Clippy. No Portless watcher is
configured for this CLI repository.

## Native releases

Tagged releases contain native archives for the platforms built by CI, plus
SHA-256 files and the first-use/protocol guides for offline reading. Compare the archive checksum against the release metadata before
extracting it. From the extracted directory run:

```sh
python3 scripts/install-release.py
```

The installer verifies every bundled file and the host platform before copying
binaries. Existing hook definitions are preserved; new definitions still need
normal Codex `/hooks` review. Credentials and hook trust are never packaged.
A previous manual installation needs `--adopt-existing` after you review it;
managed binaries that have changed are refused. Backups and the pinned version
receipt live under `~/.local/state/trapline`. Python 3.9+ suffices to install;
Rust and Nix are only build-time requirements.

## Handoff guidance

The built-in `handoff` nudge pack supplies advice for checkpoint ownership,
preflight, recovery, returned results and local trust review. Inspect it with
`nudge show handoff`; use `trapline explain session_start` for rule decisions.
The shipped Codex profile includes this pack. Known, unchanged managed profiles
upgrade through the native installer; customized profiles are preserved. Hooks
and their trust remain owned by each host. No approval settings are copied.
Both commands are also available from `adrianmross/private/trapline` on macOS.

Standalone guidance setup records its managed profile hash separately from native
binary installation. Repeated private-tap upgrades preserve customized profiles
and reuse an equivalent existing stable hook command, avoiding duplicate hooks.
