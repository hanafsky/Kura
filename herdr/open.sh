#!/bin/sh
# Launcher for the kura herdr plugin actions.
#
# Usage: open.sh split|popup|worktree
#
# Opens the kura plugin pane in the working directory of the pane that was
# focused when the action ran (`focused_pane_cwd` in HERDR_PLUGIN_CONTEXT_JSON),
# falling back to the workspace directory and then $HOME.
#
# `worktree` is the `worktree.created` event hook: only when the opt-in file
# $HERDR_PLUGIN_CONFIG_DIR/open-on-worktree-created exists, it splits kura
# beside the new worktree's pane without moving the focus. Without the file
# it does nothing (the hook runs for everyone who installed the plugin).
set -u

entry="${1:-split}"
herdr="${HERDR_BIN_PATH:-herdr}"

# Print the string value of key $1 in HERDR_PLUGIN_CONTEXT_JSON (a flat,
# single-line JSON object). Backslash escapes such as \" and \\ are undone.
context_string() {
  printf '%s\n' "${HERDR_PLUGIN_CONTEXT_JSON:-}" |
    sed -n -E "s/.*\"$1\":\"(([^\"\\\\]|\\\\.)*)\".*/\\1/p" |
    sed -E 's/\\(.)/\1/g'
}

fail() {
  echo "kura plugin: $1" >&2
  "$herdr" notification show "kura" --body "$1" >/dev/null 2>&1 || true
  exit 1
}

case "$entry" in
  split | popup) ;;
  worktree)
    [ -n "${HERDR_PLUGIN_CONFIG_DIR:-}" ] &&
      [ -e "$HERDR_PLUGIN_CONFIG_DIR/open-on-worktree-created" ] || exit 0
    ;;
  *) fail "unknown pane '$entry' (expected split, popup or worktree)" ;;
esac

command -v kura >/dev/null 2>&1 ||
  fail "kura is not on the herdr server's PATH; install it first (see README)"

cwd="$(context_string focused_pane_cwd)"
[ -n "$cwd" ] && [ -d "$cwd" ] || cwd="$(context_string workspace_cwd)"
[ -n "$cwd" ] && [ -d "$cwd" ] || cwd="$HOME"

if [ "$entry" = worktree ]; then
  # The event's pane is the new worktree's root pane, which need not be in
  # the focused workspace: target it explicitly and keep the focus.
  pane="${HERDR_PANE_ID:-}"
  [ -n "$pane" ] || pane="$(context_string focused_pane_id)"
  [ -n "$pane" ] || fail "no pane for the new worktree"
  exec "$herdr" plugin pane open --plugin kura --entrypoint split \
    --cwd "$cwd" --target-pane "$pane" --direction right --no-focus
elif [ "$entry" = split ]; then
  exec "$herdr" plugin pane open --plugin kura --entrypoint split \
    --cwd "$cwd" --direction right --focus
else
  # The CLI has no `--placement popup`; the manifest entrypoint is a popup.
  exec "$herdr" plugin pane open --plugin kura --entrypoint popup --cwd "$cwd"
fi
