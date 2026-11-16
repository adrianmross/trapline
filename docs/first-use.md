# First use: inspect guidance before enabling it

On macOS with private-tap access, install `adrianmross/private/trapline`; it
includes both trapline and nudge. Linux uses the checksum-verified native release.
Neither installation alone grants hook trust. Rust is needed only for building.

```sh
nudge packs
nudge show handoff
```

Create a local temporary profile containing `use = ["handoff"]`, then point
`TRAPLINE_CONFIG` at it for a single command:

```sh
TRAPLINE_CONFIG=/path/to/profile.toml trapline check pre handoff --format json -- result import example
TRAPLINE_CONFIG=/path/to/profile.toml trapline explain pre handoff -- result import example
```

The check evaluates advice without running the handoff command. After reviewing
the pack, add it to your own profile. For shell integration use the existing zsh
hook. For Codex merge the shipped hook groups additively, review exact definitions
with `/hooks`, and test in a disposable new session. Existing sessions and custom
profiles are preserved. Hook trust stays local to each host. Guidance is not a
security boundary and never authorizes execution.

## Team guidance

The built-in handoff pack notices `.handoff.json` at session start and points to
explicit `handoff project inspect`/adoption. It also reminds users to independently
test imported results. It does not parse the project file as rules or load any
repository-owned handlers.

Teams can distribute a reviewed TOML pack through their existing private
configuration distribution. Copy it to the user pack directory only after review
and explicitly include its name in a local profile. Recommended pack names in a
project manifest install nothing. Do not enable arbitrary repository handlers
implicitly; existing packs/handlers/adapters are sufficient extension points.

## Reduce repeated advice

For binary checks and agent hooks, opt in with `TRAPLINE_HINTS_ONCE=1` and a
session identifier (`TRAPLINE_SESSION_ID`, or `CODEX_THREAD_ID`). Export a fresh
identifier for each shell session if using it manually. Identical hints in the
same directory/session appear once. Changed messages and new sessions appear
again. Warnings, denials, wrappers and all other decision effects remain intact.
Without a session identifier or if state cannot be saved, hints remain visible.

The bounded local cache contains only hashed metadata under
`$XDG_STATE_HOME/trapline/hints` (default `~/.local/state/trapline/hints`), with
private permissions. Once full, it stops suppressing new hints. Removing that
cache merely makes hints visible again. Compiled zsh rules do not invoke the
binary; use `trapline hook zsh --no-compile` if you want this suppression in shell
wrappers. `trapline explain` always shows matching rules regardless of suppression.

For automation evaluate explicit events through `trapline eval` or use
`trapline check ... --format json`. Unsupported/untrusted hooks do not demonstrate
coverage; test the actual agent hook locally. Keep full event inputs and logs
private because command paths and repository names can identify internal work.
