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

`phase` is one of `pre`, `post`, `notfound`, `chpwd`, `prompt`. `tool` and
`args` describe the wrapped command (`tool` is the missing name for
`notfound`). `rc` is set for `post`.

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
- `fired` names the rules that matched, for explaining a decision.

## CLI forms

`trapline check <phase> [tool] [--rc N] [--cwd DIR] [--format plain|json|shell] [-- args]`

- `plain`: stdout is the prefix, one argument per line; messages go to stderr;
  exit 1 blocks.
- `json`: the decision above.
- `shell`: `export K='V'` / `unset K` lines, for prompt markers.
