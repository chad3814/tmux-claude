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

# (allowlist matching + rewrite added in later tasks)
passthrough
