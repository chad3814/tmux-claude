#!/usr/bin/env bash
# route-to-nrun.sh — Claude Code PreToolUse hook.
#
# Transparently routes allowlisted long-lived commands (dev servers, docker,
# build tools) through `nrun` so they run in a dedicated tmux window while
# Claude still reads their output. Anything unrecognized runs unchanged.
#
# Fail-open: on ANY error, unrecognized input, or non-match, print nothing and
# exit 0 so the original command runs exactly as issued.

set -u

passthrough() { exit 0; }

# Dependencies. Without jq we cannot parse/emit JSON safely.
command -v jq >/dev/null 2>&1 || passthrough

input="$(cat)" || passthrough

tool_name="$(printf '%s' "$input" | jq -r '.tool_name // empty' 2>/dev/null)" || passthrough
[ "$tool_name" = "Bash" ] || passthrough

cmd="$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null)" || passthrough
[ -n "$cmd" ] || passthrough

# nrun must be installed; otherwise routing is impossible.
command -v nrun >/dev/null 2>&1 || passthrough

# --- tokenize ----------------------------------------------------------------
read -r -a toks <<< "$cmd"
[ "${#toks[@]}" -ge 1 ] || passthrough

# transparent `npx` prefix
if [ "${toks[0]}" = "npx" ] && [ "${#toks[@]}" -ge 2 ]; then
    eff=("${toks[@]:1}")
else
    eff=("${toks[@]}")
fi
eprog="${eff[0]}"

# recursion guard
[ "$eprog" = "nrun" ] && passthrough

contains() {
    local needle="$1"
    shift
    local x
    for x in "$@"; do [ "$x" = "$needle" ] && return 0; done
    return 1
}

# --- allowlist ---------------------------------------------------------------
# Membership (`contains`) is used instead of positional matching on purpose: the
# `dev` script can sit after flags (e.g. `pnpm --filter web dev`, and for npm
# `npm --workspace web run dev`), so a fixed position would miss real
# invocations. The only over-match is a nonsense ordering like `npm dev run`,
# which is harmless (it fails the same whether routed through nrun or not).
matched=0
case "$eprog" in
    pnpm) contains dev "${eff[@]:1}" && matched=1 ;;
    npm) contains run "${eff[@]:1}" && contains dev "${eff[@]:1}" && matched=1 ;;
    yarn) contains dev "${eff[@]:1}" && matched=1 ;;
    next) contains dev "${eff[@]:1}" && matched=1 ;;
    vite) matched=1 ;;
    make | cmake) matched=1 ;;
    configure | ./configure) matched=1 ;;
esac
[ "$matched" = "1" ] || passthrough

title="$eprog"
effective_cmd="$cmd"

# --- write the temp script (no nested quoting in the rewritten command) ------
script="$(mktemp "${TMPDIR:-/tmp}/nrun-script-XXXXXX.sh")" || passthrough
{
    printf '#!/usr/bin/env bash\n'
    printf 'exec %s\n' "$effective_cmd"
} >"$script" || passthrough
chmod +x "$script" 2>/dev/null || true

# --- build the nrun command --------------------------------------------------
nrun_cmd="nrun --title $(printf '%q' "$title") $(printf '%q' "$script")"

# --- emit updatedInput, preserving all original tool_input fields ------------
printf '%s' "$input" | jq -c --arg cmd "$nrun_cmd" \
    '{hookSpecificOutput: {hookEventName: "PreToolUse", updatedInput: (.tool_input + {command: $cmd, run_in_background: true})}}' \
    2>/dev/null || passthrough
