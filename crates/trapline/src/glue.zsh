# trapline shell glue. Add to .zshrc:  eval "$(trapline hook zsh)"
autoload -Uz add-zsh-hook

# Prompt markers: prepended so they are set before the prompt renders.
_trapline_precmd() { eval "$(command trapline check prompt --format shell 2>/dev/null)" }
precmd_functions=(_trapline_precmd ${precmd_functions:#_trapline_precmd})

# Defaults call the binary; `trapline hook zsh` appends compiled versions of
# these (pure zsh, no process) when every rule in the phase can be compiled.
_trapline_chpwd() { command trapline check chpwd --format shell >/dev/null }
_trapline_notfound_hint() { command trapline check notfound "$1" >/dev/null }
add-zsh-hook chpwd _trapline_chpwd

# Pre/post default to calling the binary; `trapline hook zsh` appends compiled
# versions (pure zsh) when every rule can be compiled. `_trapline_pre` sets
# `_tl_prefix` (argv to run the tool under) and returns 1 to block.
_trapline_pre() {
  local tool="$1"; shift
  _tl_prefix=("${(@f)$(command trapline check pre "$tool" -- "$@")}") || return 1
  [[ -n "${_tl_prefix[1]}" ]] || _tl_prefix=()
}
_trapline_post() { command trapline check post "$1" --rc "$2" -- "${@:3}" >/dev/null }

# Run a wrapped tool: ask for a prefix (e.g. `devenv shell --`), run, report.
# A function the tool already had is kept (as _trapline_orig_<tool>) and called.
_trapline_run() {
  local tool="$1"; shift
  local -a _tl_prefix
  _trapline_pre "$tool" "$@" || return 1
  if (( ${#_tl_prefix} )); then "${_tl_prefix[@]}" "$tool" "$@"
  elif (( $+functions[_trapline_orig_$tool] )); then "_trapline_orig_$tool" "$@"
  else command "$tool" "$@"; fi
  local rc=$?
  _trapline_post "$tool" $rc "$@"
  return $rc
}

# Hint first, then the original handler (a Homebrew lookup can be slow).
# Safe to call again: when a plugin has replaced the handler since, the new one
# is captured and wrapped.
_trapline_wrap_cnf() {
  [[ "${functions[command_not_found_handler]}" == *_trapline_orig_cnf* ]] && return 0
  unfunction _trapline_orig_cnf 2>/dev/null
  if (( $+functions[command_not_found_handler] )); then
    functions -c command_not_found_handler _trapline_orig_cnf
  else
    _trapline_orig_cnf() { print -u2 "zsh: command not found: $1"; return 127 }
  fi
  command_not_found_handler() {
    _trapline_notfound_hint "$1"
    _trapline_orig_cnf "$@"
  }
}
_trapline_wrap_cnf
