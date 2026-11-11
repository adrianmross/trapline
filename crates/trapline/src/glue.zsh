# trapline shell glue. Add to .zshrc:  eval "$(trapline hook zsh)"
autoload -Uz add-zsh-hook

# Prompt markers: prepended so they are set before the prompt renders.
_trapline_precmd() { eval "$(command trapline check prompt --format shell 2>/dev/null)" }
precmd_functions=(_trapline_precmd ${precmd_functions:#_trapline_precmd})

_trapline_chpwd() { command trapline check chpwd --format shell >/dev/null }
add-zsh-hook chpwd _trapline_chpwd

# Run a wrapped tool: ask for a prefix (e.g. `devenv shell --`), run, report.
# A function the tool already had is kept (as _trapline_orig_<tool>) and called.
_trapline_run() {
  local tool="$1"; shift
  local -a p
  p=("${(@f)$(command trapline check pre "$tool" -- "$@")}") || return 1
  [[ -n "${p[1]}" ]] || p=()
  if (( ${#p} )); then "${p[@]}" "$tool" "$@"
  elif (( $+functions[_trapline_orig_$tool] )); then "_trapline_orig_$tool" "$@"
  else command "$tool" "$@"; fi
  local rc=$?
  command trapline check post "$tool" --rc $rc -- "$@" >/dev/null
  return $rc
}

# Hint first, then the original handler (a Homebrew lookup can be slow).
# Call again after plugins that define their own handler have loaded.
_trapline_wrap_cnf() {
  (( $+functions[_trapline_orig_cnf] )) && return 0
  if (( $+functions[command_not_found_handler] )); then
    functions -c command_not_found_handler _trapline_orig_cnf
  else
    _trapline_orig_cnf() { print -u2 "zsh: command not found: $1"; return 127 }
  fi
  command_not_found_handler() {
    command trapline check notfound "$1" >/dev/null
    _trapline_orig_cnf "$@"
  }
}
_trapline_wrap_cnf
