# Shell integration for cs-fzf. Source this, then run `csf`.
#
#   echo 'source ~/dev/projects/chat-search/scripts/cs-fzf.sh' >> ~/.zshrc
#
# `csf` is a *function*, not a script, so the agent it launches is an ordinary child of
# your interactive shell and inherits the terminal exactly as if you had typed the command.
# That is the whole point: running the agent from inside the search script instead means
# handing a full-screen TUI a terminal across an exec, which is where Claude Code stops
# accepting keystrokes and Codex panics in crossterm with "reader source not set".
#
# Works in both zsh and bash.

if [ -n "${ZSH_VERSION:-}" ]; then
  # In zsh, $0 inside a sourced file is that file's path (FUNCTION_ARGZERO, on by default).
  _CS_FZF_DIR="${0:A:h}"
else
  _CS_FZF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fi
export _CS_FZF_DIR

csf() {
  local cmd
  cmd="$("$_CS_FZF_DIR/cs-fzf")" || return 0
  [ -n "$cmd" ] || return 0

  # Put it in history first, so the session is re-runnable with ↑ after you exit the agent.
  if [ -n "${ZSH_VERSION:-}" ]; then
    print -s -- "$cmd"
  else
    history -s -- "$cmd"
  fi

  printf '\033[2m%s\033[0m\n' "$cmd"
  eval "$cmd"
}
