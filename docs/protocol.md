# Handler protocol

A handler turns an `Event` into a `Decision`. `nudge-engine` implements it in
process; anything that reads the JSON below on stdin and writes a decision on
stdout can be a handler. The types live in `trapline-core`.

## Event

```json
{
  "phase": "pre",
  "tool": "nvim",
  "args": ["README.md"],
  "cwd": "/home/me/repo/sub",
  "env": { "DEVENV_ROOT": "/home/me/repo" },
  "rc": null
}
```

`phase` is one of `pre`, `post`, `notfound`, `chpwd`, `prompt`, `session_start`, `sandbox`, `tool`. `tool` and
`args` describe the wrapped command (`tool` is the missing name for
`notfound`). `rc` is set for `post`. In `tool`, an agent is calling one of its
own tools: `tool` is the tool's name and `args` are its input's top-level
fields as `key=value` (strings bare, other values as JSON, null and empty
strings left out), so `args_has = ["isolation=worktree"]` matches a value and
`args_has = ["name"]` / `args_lacks = ["path"]` test whether a field is set.

## Decision

```json
{
  "proceed": true,
  "prefix": ["devenv", "shell", "--no-tui", "--"],
  "messages": [{ "level": "hint", "text": "nvim may live in this repo's devenv" }],
  "set_env": { "DEVENV_OFF": "⬡ off", "DEVENV_ON": null },
  "fired": ["devenv-launch"]
}
```

- `proceed: false` blocks the command.
- `prefix` is the argv to run the command under.
- `set_env` lists every key the handler can set; `null` means unset, so a stale
  marker clears.
- `fs` (sandbox phase) lists `deny_write`, `allow_write` and `lock_write` paths;
  each tier overrides the one before it.
- `fired` names the rules that matched, for explaining a decision.

## CLI forms

`trapline check <phase> [tool] [--rc N] [--cwd DIR] [--format plain|json|shell] [-- args]`

- `plain`: stdout is the prefix, one argument per line; messages go to stderr;
  exit 1 blocks.
- `json`: the decision above.
- `shell`: `export K='V'` / `unset K` lines, for prompt markers.

## Agent hooks

`trapline agent claude` reads a Claude Code hook payload on stdin and prints the
hook response on stdout (nothing when there is nothing to say; always exit 0).

| Hook | Becomes | Response |
|---|---|---|
| `SessionStart` | one `session_start` event | `additionalContext` (and `systemMessage`) with the messages |
| `PreToolUse`, `Bash` | one `pre` event per simple command | `block` -> `permissionDecision: "deny"` with the reason; otherwise `additionalContext` only, so the permission flow is unchanged |
| `PreToolUse`, any other tool | one `tool` event (`tool` = `tool_name`, `args` from `tool_input`) | as for `Bash` |
| anything else | none | no output |

## Evaluating from your own code

`trapline eval` reads one Event as JSON on stdin and prints the Decision as
JSON. External handlers use the same two shapes in the other direction: they
receive an Event and may reply with any subset of Decision fields (`proceed`
defaults to true).
