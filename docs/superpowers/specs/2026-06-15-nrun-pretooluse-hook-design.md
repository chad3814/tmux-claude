# Design: PreToolUse hook to route dev servers/containers through `nrun`

- **Issue:** #2
- **Branch:** `feat/2-nrun-hook`
- **Date:** 2026-06-15
- **Depends on:** #1 (`nrun`, merged)
- **Split out:** #3 (SessionEnd orphan sweep — deliberately out of scope here)

## Goal

Make `nrun` fire automatically. A `PreToolUse` hook on the `Bash` tool transparently routes
allowlisted long-lived commands into a dedicated tmux window via `nrun`, so the user gets a
watchable per-service window while Claude still reads the command's output normally. Any command
the hook doesn't recognize runs exactly as Claude issued it.

## Scope

**In scope:** the PreToolUse hook script, its match/rewrite logic, docker rewrite rules, title
derivation, guards, Rust-driven tests, and documented `settings.json` wiring.

**Out of scope (issue #3):** the `SessionEnd` orphan sweep and the `nrun` registry/state-file it
requires. `nrun` already handles every *graceful* teardown path; the only deferred gap is the
macOS hard-`SIGKILL` orphan case.

## Verified hook contract

Confirmed against the current Claude Code hooks reference
(<https://code.claude.com/docs/en/hooks.md>):

**Input (stdin JSON):**

```json
{
  "session_id": "…",
  "cwd": "/abs/path",
  "permission_mode": "default",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "pnpm dev",
    "description": "…",
    "timeout": 120000,
    "run_in_background": false
  }
}
```

**Output to rewrite (stdout JSON):** emit `updatedInput` **without** `permissionDecision`, so the
rewrite is applied *and* the rewritten command still flows through the user's normal permission
system (no silent auto-approve):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "updatedInput": {
      "command": "nrun --title web /tmp/nrun-script-…sh",
      "description": "…",
      "timeout": 120000,
      "run_in_background": true
    }
  }
}
```

- `updatedInput` **completely replaces** `tool_input` — all original fields (`description`,
  `timeout`, …) must be carried over; only `command` and `run_in_background` are changed.
- **Pass-through** = print nothing, `exit 0`. This defers to the normal permission system with no
  modification.

## Architecture & data flow

A single committed shell script, `hooks/route-to-nrun.sh`, wired as a `PreToolUse` hook matched to
`Bash` in `~/.claude/settings.json`.

```
stdin JSON  ──▶  route-to-nrun.sh  ──▶  stdout
{tool_name, tool_input:{command,…}}      rewrite JSON  (intercept)
                                         nothing + exit 0  (pass through)
```

**Fail-open is the core invariant.** The hook fires on *every* Bash call in *every* session, so the
default and the response to any error (bad JSON, unwritable tempdir, unexpected shape, missing
`nrun`, no match) is to pass through untouched. False negatives (failing to intercept) are
harmless; false positives (mangling a command) are the only real danger, so matching is
deliberately conservative — when in doubt, pass through.

## Components

### 1. Match → rewrite pipeline (`route-to-nrun.sh`)

In order, any step may short-circuit to pass-through:

1. If `tool_name != "Bash"` → pass through.
2. Extract `tool_input.command`. Strip a single optional leading `cd <path> &&` and any leading
   `VAR=value` env assignments; remember them for the generated script.
3. **Disqualify compound shapes:** if the remainder contains a pipe `|`, redirect `<`/`>`, `;`,
   another `&&`/`||`, background `&`, subshell `(`/`)`, backtick, or `$(` → pass through. (The one
   allowed `&&` is the leading `cd` stripped in step 2.)
4. **Allowlist match** on the remaining program + subcommand (see Allowlist). No match → pass
   through.
5. **Recursion guard:** if the program is already `nrun` → pass through. Also pass through if
   `command -v nrun` fails (nrun not installed).
6. Derive the **title**; if `docker run`, compute the **docker rewrite** and container name.
7. Write the **temp script**; emit the rewrite JSON with `run_in_background: true`.

### 2. Allowlist

Matched on the de-prefixed command:

| Pattern | Notes |
|---|---|
| `pnpm dev`, `pnpm run dev`, `pnpm --filter X dev` / `pnpm -F X dev` | `dev` is the script |
| `npm run dev` | |
| `yarn dev` | |
| `next dev`, `npx next dev` | |
| `vite`, `npx vite` | |
| `docker run …` | triggers docker rewrite |
| `docker compose up`, `docker-compose up` | compose lifecycle (no `--rm`/`--name`) |
| `make [target]` | short-lived; runs to completion in window |
| `cmake …` | short-lived |
| `configure`, `./configure` | short-lived |

Server commands run forever; build commands (`make`/`cmake`/`configure`) exit on their own. Both
are handled **identically** — see Background behavior.

### 3. Background behavior (uniform)

Every intercepted command is rewritten with `run_in_background: true`. A never-exiting dev server
would otherwise hang the foreground tool call; build tools simply run to completion in the window,
Claude is notified when they finish, and the window closes on exit. One code path, no
server-vs-build classification.

### 4. Title derivation

Priority order:

1. `--filter X` / `-F X` value (pnpm monorepo) → `X` (e.g. `web`, `app`).
2. docker image basename with registry/tag stripped (`postgres:16` → `postgres`,
   `ghcr.io/foo/bar:1` → `bar`).
3. fall back to the program name (`pnpm`, `vite`, `next`, `make`, `cmake`, `configure`, `docker`).
   `docker compose`/`docker-compose` → `compose`.

`nrun` sanitizes the title for its logfile name on its own, so any residual unsafe characters are
handled downstream.

### 5. Docker rewrite (`docker run` only)

- drop `-d` / `--detach`
- ensure `--rm`
- keep `-t`; keep `-i` only if already present
- ensure `--name <stable>`: reuse the user's `--name` if present, else derive from the image
  basename. Pass the same value to `nrun --docker-name <name>` so teardown does
  `docker stop <name>`.

`docker compose up` / `docker-compose up`: drop `-d` only; no `--rm`/`--name`/`--docker-name`
(compose owns its container lifecycle). Title `compose`.

### 6. Temp script generation

Write to `${TMPDIR:-/tmp}/nrun-script-$$-<n>.sh` so the rewritten tool command contains no nested
quoting:

```sh
#!/usr/bin/env bash
cd /the/folded/path          # only if a leading `cd …` was stripped
export FORCE_COLOR=1         # only if env prefixes were stripped
exec pnpm --filter web dev   # the effective (possibly docker-rewritten) command, exec'd
```

`exec` makes the tmux pane's `#{pane_pid}` resolve to the real process (nrun relies on this for
signalling/liveness).

Rewritten `tool_input` = the original object with **all fields preserved**, overriding only:
- `command` → `nrun --title <T> [--docker-name <N>] <script-path>`
- `run_in_background` → `true`

## Error handling

Every failure path passes through (prints nothing, `exit 0`):

- non-Bash tool, unparseable JSON, missing `command`, unexpected shape
- compound/disqualified command shape
- no allowlist match
- `nrun` already the program (recursion) or `nrun` not on `PATH`
- inability to create the temp script

The hook never emits `permissionDecision`, so it never blocks a command and never auto-approves
one; it only ever rewrites-and-defers or passes through.

## Testing — Rust-driven (`tests/hook_integration.rs`)

Driven by the existing `cargo test` gate. Each test spawns `hooks/route-to-nrun.sh` via
`std::process::Command`, pipes a JSON fixture to stdin, and asserts on stdout. A test fixture puts
a dummy `nrun` executable on `PATH` so `command -v nrun` is deterministic; one case removes it to
assert pass-through when `nrun` is absent.

**Intercept cases**
- `pnpm dev` → command becomes `nrun --title pnpm … <script>`, `run_in_background: true`; script
  ends `exec pnpm dev`.
- `pnpm --filter web dev` → title `web`.
- `cd app && pnpm dev` → script folds `cd app`, ends `exec pnpm dev`.
- `FORCE_COLOR=1 vite` → script exports the env var, ends `exec vite`.
- `docker run -d --name pg postgres:16` → `-d` dropped, `--rm` ensured, `--docker-name pg`, title
  `postgres`.
- `docker run -d nginx` → name derived `nginx`, title `nginx`.
- `docker compose up -d` → `-d` dropped, no `--name`/`--docker-name`, title `compose`.
- `make build` → intercepted, `run_in_background: true`.

**Pass-through cases**
- `pnpm dev | grep ready`, `a && b && pnpm dev`, `pnpm dev > out.log` (compound/disqualified)
- `ls`, `npm test`, `git status` (not allowlisted)
- `nrun --title x s.sh` (recursion guard)
- malformed JSON on stdin → empty stdout, exit 0 (fail-open)
- `tool_name: "Read"` → pass through
- `nrun` absent from `PATH` → pass through

**Field-preservation case**
- input `tool_input` carrying `description` and `timeout` → both present unchanged in
  `updatedInput`.

## Wiring (documented, not auto-applied)

The build will **not** edit `~/.claude/settings.json`. The spec/README documents the opt-in block:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "/abs/path/to/tmux-claude/<worktree>/hooks/route-to-nrun.sh" }
        ]
      }
    ]
  }
}
```

`nrun` must be on `PATH` (`cargo install --path .`). If it isn't, the hook passes everything
through, so the only consequence of a missing install is "no routing," never a broken command.

## Acceptance

- Allowlisted command in a tmux session → runs in a new window, output streamed back to Claude,
  window cleaned up on completion/stop (via `nrun`'s existing graceful teardown).
- Non-allowlisted or compound command → unchanged.
- `docker run` → `-d` dropped, `--rm --name` injected, container stopped on teardown.
- Hook is fail-open: any error or unrecognized input passes the command through verbatim.
- `cargo test` (incl. the new hook integration tests), `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo build --release` all pass.
