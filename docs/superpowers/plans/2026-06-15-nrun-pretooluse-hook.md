# nrun PreToolUse Hook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a fail-open `PreToolUse` hook that transparently routes allowlisted long-lived commands (dev servers, docker, build tools) through `nrun` into a dedicated tmux window, while leaving every other command untouched.

**Architecture:** A single committed bash script `hooks/route-to-nrun.sh` reads the hook event JSON on stdin, and either prints nothing + exits 0 (pass through) or prints `{hookSpecificOutput.updatedInput}` rewriting the command to `nrun … <script>` with `run_in_background: true`. It is fail-open: any error, unrecognized input, or non-match passes through. Tests are Rust-driven (`tests/hook_integration.rs`) so they run under the existing `cargo test` gate; each spawns the hook in an isolated PATH (symlinked tools + optional dummy `nrun`), feeds a JSON fixture, and asserts on stdout.

**Tech Stack:** Bash + `jq` (hook); Rust + `serde_json` dev-dependency (tests); `nrun` (existing binary, must be on PATH at runtime).

**Spec:** `docs/superpowers/specs/2026-06-15-nrun-pretooluse-hook-design.md`

---

## File structure

- `hooks/route-to-nrun.sh` — the entire hook. One responsibility: classify a Bash command and emit a rewrite-or-passthrough decision. Built up incrementally across Tasks 1–5; each task shows the **complete** script as it stands.
- `tests/hook_integration.rs` — Rust harness + black-box test cases. Grows one block per task.
- `Cargo.toml` — add `serde_json` under `[dev-dependencies]` (Task 1). Release binary stays dependency-unchanged.
- `README.md` — opt-in `~/.claude/settings.json` wiring instructions (Task 6).

**Commit gate (must pass before every commit, per project CLAUDE.md):**
`cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test`; `cargo build --release`.

**Harness contract note (added after Task 1 review):** `run_hook` returns `(String, TestRoot)`, not `String` — the second element is an RAII guard that deletes the isolated temp dir on drop. Every test must bind it to a named local so the temp dir (and any generated script the hook wrote under `TMPDIR`) survives until the assertions finish:

```rust
let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
// ... assert on `out`; call script_body(&rewritten_command(&out)) while `_root` is in scope ...
```

For single-line pass-through assertions, bind first: `let (out, _root) = run_hook(input, true); assert!(out.trim().is_empty());`. The Task 2–5 test snippets below are written in the older `let out = run_hook(...)` form — **adapt each to the `(out, _root)` tuple form.**

---

## Task 1: Harness + fail-open skeleton

Establishes the test harness and a skeleton hook that only parses the event and always passes through. The pass-through behaviors are real (safe) behavior, so these tests are green immediately.

**Files:**
- Create: `hooks/route-to-nrun.sh`
- Create: `tests/hook_integration.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `serde_json` dev-dependency**

Edit `Cargo.toml` — add (or create) a `[dev-dependencies]` section. Leave existing `[dependencies]` untouched:

```toml
[dev-dependencies]
serde_json = "1"
```

- [ ] **Step 2: Write the skeleton hook**

Create `hooks/route-to-nrun.sh`:

```bash
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
```

- [ ] **Step 3: Make the hook executable**

Run: `chmod +x hooks/route-to-nrun.sh`
Expected: no output, exit 0.

- [ ] **Step 4: Write the harness + the four pass-through tests**

Create `tests/hook_integration.rs`:

```rust
//! Black-box integration tests for the PreToolUse hook `hooks/route-to-nrun.sh`.
//!
//! Each test runs the hook in an isolated PATH containing only symlinks to the
//! tools the hook needs (plus, optionally, a dummy `nrun`), feeds a JSON event
//! on stdin, and asserts on the JSON (or empty) stdout.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn hook_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("hooks/route-to-nrun.sh")
}

/// Resolve an executable's absolute path via `command -v`.
fn which(tool: &str) -> Option<PathBuf> {
    let out = Command::new("/usr/bin/env")
        .args(["sh", "-c", &format!("command -v {tool}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() {
        None
    } else {
        Some(PathBuf::from(p))
    }
}

/// Build an isolated test root with a `bin/` of symlinks to the tools the hook
/// needs. If `with_nrun`, add a dummy `nrun` that exits 0.
fn make_env(with_nrun: bool) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("nrun-hooktest-{}-{}", std::process::id(), n));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in ["env", "bash", "sh", "jq", "mktemp", "chmod", "cat"] {
        if let Some(src) = which(tool) {
            let _ = symlink(&src, bin.join(tool));
        }
    }
    if with_nrun {
        let nrun = bin.join("nrun");
        fs::write(&nrun, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perm = fs::metadata(&nrun).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&nrun, perm).unwrap();
    }
    root
}

/// Run the hook with `stdin` in an isolated PATH. Returns stdout. Asserts exit 0.
fn run_hook(stdin: &str, with_nrun: bool) -> String {
    let root = make_env(with_nrun);
    let bin = root.join("bin");
    let mut child = Command::new(hook_path())
        .env("PATH", &bin)
        .env("TMPDIR", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait hook");
    assert!(
        out.status.success(),
        "hook must always exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

/// Build a Bash-tool PreToolUse event JSON for `command`.
fn bash_event(command: &str) -> String {
    serde_json::json!({
        "session_id": "s", "cwd": "/tmp", "permission_mode": "default",
        "hook_event_name": "PreToolUse", "tool_name": "Bash",
        "tool_input": {
            "command": command, "description": "d",
            "timeout": 120000, "run_in_background": false
        }
    })
    .to_string()
}

/// Parse hook stdout into the `updatedInput` object; None if pass-through (empty).
#[allow(dead_code)]
fn updated_input(stdout: &str) -> Option<Value> {
    let s = stdout.trim();
    if s.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(s).expect("valid JSON out");
    Some(v["hookSpecificOutput"]["updatedInput"].clone())
}

/// The rewritten command string from a rewrite decision.
#[allow(dead_code)]
fn rewritten_command(stdout: &str) -> String {
    updated_input(stdout).expect("expected a rewrite")["command"]
        .as_str()
        .expect("command is a string")
        .to_string()
}

/// Read the temp script referenced as the last token of an `nrun … <script>` command.
#[allow(dead_code)]
fn script_body(nrun_command: &str) -> String {
    let path = nrun_command.split_whitespace().last().unwrap();
    fs::read_to_string(path).unwrap()
}

#[test]
fn non_bash_tool_passes_through() {
    let event = serde_json::json!({
        "tool_name": "Read", "tool_input": {"file_path": "/x"}
    })
    .to_string();
    assert!(run_hook(&event, true).trim().is_empty());
}

#[test]
fn malformed_json_passes_through() {
    assert!(run_hook("{ this is not json", true).trim().is_empty());
}

#[test]
fn empty_command_passes_through() {
    assert!(run_hook(&bash_event(""), true).trim().is_empty());
}

#[test]
fn nrun_absent_passes_through() {
    assert!(run_hook(&bash_event("pnpm dev"), false).trim().is_empty());
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --test hook_integration`
Expected: PASS — 4 passed. (Skeleton always passes through, which is exactly what these assert.)

- [ ] **Step 6: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml hooks/route-to-nrun.sh tests/hook_integration.rs
git commit -m "Add fail-open PreToolUse hook skeleton + test harness (#2)"
```

---

## Task 2: Core intercept — allowlist + rewrite machinery

Match simple allowlisted commands and rewrite them to `nrun … <script>`. No cd/env folding or disqualifier yet (added in Task 3); `effective_cmd` is the whole command.

**Files:**
- Modify: `hooks/route-to-nrun.sh`
- Modify: `tests/hook_integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/hook_integration.rs`:

```rust
#[test]
fn pnpm_dev_is_rewritten_to_nrun() {
    let out = run_hook(&bash_event("pnpm dev"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.starts_with("nrun --title pnpm "), "got: {cmd}");
    let ui = updated_input(&out).unwrap();
    assert_eq!(ui["run_in_background"], serde_json::json!(true));
}

#[test]
fn rewrite_preserves_original_tool_input_fields() {
    let out = run_hook(&bash_event("pnpm dev"), true);
    let ui = updated_input(&out).unwrap();
    assert_eq!(ui["description"], serde_json::json!("d"));
    assert_eq!(ui["timeout"], serde_json::json!(120000));
}

#[test]
fn generated_script_execs_the_command() {
    let out = run_hook(&bash_event("pnpm dev"), true);
    let body = script_body(&rewritten_command(&out));
    assert!(body.contains("exec pnpm dev"), "script was: {body}");
}

#[test]
fn simple_allowlist_entries_are_intercepted() {
    for (input, title) in [
        ("vite", "vite"),
        ("make", "make"),
        ("cmake", "cmake"),
        ("configure", "configure"),
        ("npm run dev", "npm"),
        ("yarn dev", "yarn"),
        ("next dev", "next"),
        ("npx next dev", "next"),
        ("npx vite", "vite"),
    ] {
        let out = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(
            cmd.contains(&format!("--title {title} ")),
            "input {input:?} -> {cmd}"
        );
    }
}

#[test]
fn non_allowlisted_commands_pass_through() {
    for input in ["ls", "npm test", "git status", "echo pnpm dev"] {
        assert!(
            run_hook(&bash_event(input), true).trim().is_empty(),
            "expected pass-through for {input:?}"
        );
    }
}

#[test]
fn recursion_guard_passes_nrun_through() {
    assert!(run_hook(&bash_event("nrun --title x /tmp/s.sh"), true)
        .trim()
        .is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test hook_integration`
Expected: FAIL — the new tests panic in `rewritten_command` ("expected a rewrite") because the skeleton still passes through. (The 4 Task-1 tests still pass.)

- [ ] **Step 3: Implement the core in the hook**

Replace the final two lines of `hooks/route-to-nrun.sh` (the comment + `passthrough`) with:

```bash
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test hook_integration`
Expected: PASS — all tests pass.

- [ ] **Step 5: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 6: Commit**

```bash
git add hooks/route-to-nrun.sh tests/hook_integration.rs
git commit -m "Intercept simple allowlisted commands, rewrite to nrun (#2)"
```

---

## Task 3: Command normalization — disqualifier + cd/env folding

Reject compound shapes (pipes, redirects, subshells, sequences, command substitution) and fold a leading `cd <dir> &&` plus leading `VAR=value` env assignments into the generated script.

**Files:**
- Modify: `hooks/route-to-nrun.sh`
- Modify: `tests/hook_integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/hook_integration.rs`:

```rust
#[test]
fn compound_shapes_pass_through() {
    for input in [
        "pnpm dev | grep ready",
        "a && b && pnpm dev",
        "pnpm dev > out.log",
        "pnpm dev < in.txt",
        "pnpm dev; echo done",
        "(pnpm dev)",
        "echo `pnpm dev`",
        "pnpm dev &",
    ] {
        assert!(
            run_hook(&bash_event(input), true).trim().is_empty(),
            "expected pass-through for {input:?}"
        );
    }
}

#[test]
fn leading_cd_is_folded_into_script() {
    let out = run_hook(&bash_event("cd app && pnpm dev"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title pnpm "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("cd app"), "script was: {body}");
    assert!(body.contains("exec pnpm dev"), "script was: {body}");
}

#[test]
fn leading_env_assignments_are_folded_into_script() {
    let out = run_hook(&bash_event("FORCE_COLOR=1 vite"), true);
    let body = script_body(&rewritten_command(&out));
    assert!(body.contains("export FORCE_COLOR=1"), "script was: {body}");
    assert!(body.contains("exec vite"), "script was: {body}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test hook_integration`
Expected: FAIL — `compound_shapes_pass_through` fails (e.g. `pnpm dev | grep ready` is currently intercepted), and the cd/env tests fail (no `cd app` / `export` line in the script).

- [ ] **Step 3: Insert normalization before tokenizing**

In `hooks/route-to-nrun.sh`, immediately **after** the `command -v nrun … || passthrough` line and **before** the `# --- tokenize ---` block, insert:

```bash
# --- reject command-substitution / pipes / redirects / subshells / sequences -
# (A leading `cd <dir> &&` is the only `&` we tolerate; handled just below.)
case "$cmd" in
    *'|'* | *';'* | *'<'* | *'>'* | *'('* | *')'* | *'`'*) passthrough ;;
esac

# --- strip a single leading `cd <dir> &&` ------------------------------------
cd_dir=""
if [[ "$cmd" =~ ^[[:space:]]*cd[[:space:]]+([^[:space:]&]+)[[:space:]]*\&\&[[:space:]]*(.+)$ ]]; then
    cd_dir="${BASH_REMATCH[1]}"
    cmd="${BASH_REMATCH[2]}"
fi

# --- strip leading VAR=value env assignments ---------------------------------
env_assignments=()
while [[ "$cmd" =~ ^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]&]*)[[:space:]]+(.+)$ ]]; do
    env_assignments+=("${BASH_REMATCH[1]}")
    cmd="${BASH_REMATCH[2]}"
done

# trim surrounding whitespace
cmd="${cmd#"${cmd%%[![:space:]]*}"}"
cmd="${cmd%"${cmd##*[![:space:]]}"}"

# any remaining `&` is a background/stray operator we do not handle
case "$cmd" in *'&'*) passthrough ;; esac
[ -n "$cmd" ] || passthrough
```

- [ ] **Step 4: Emit the cd/env lines in the generated script**

In the script-writing block, replace:

```bash
{
    printf '#!/usr/bin/env bash\n'
    printf 'exec %s\n' "$effective_cmd"
} >"$script" || passthrough
```

with:

```bash
{
    printf '#!/usr/bin/env bash\n'
    [ -n "$cd_dir" ] && printf 'cd %q\n' "$cd_dir"
    for kv in ${env_assignments[@]+"${env_assignments[@]}"}; do
        printf 'export %s\n' "$kv"
    done
    printf 'exec %s\n' "$effective_cmd"
} >"$script" || passthrough
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test hook_integration`
Expected: PASS — all tests pass.

- [ ] **Step 6: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 7: Commit**

```bash
git add hooks/route-to-nrun.sh tests/hook_integration.rs
git commit -m "Reject compound commands; fold leading cd/env into script (#2)"
```

---

## Task 4: Title derivation from `--filter`

Use a pnpm `--filter`/`-F` value as the window title when present; otherwise keep the program-name fallback.

**Files:**
- Modify: `hooks/route-to-nrun.sh`
- Modify: `tests/hook_integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/hook_integration.rs`:

```rust
#[test]
fn filter_value_becomes_title() {
    for (input, title) in [
        ("pnpm --filter web dev", "web"),
        ("pnpm -F app dev", "app"),
        ("pnpm --filter=api dev", "api"),
    ] {
        let out = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(
            cmd.contains(&format!("--title {title} ")),
            "input {input:?} -> {cmd}"
        );
    }
}

#[test]
fn without_filter_title_falls_back_to_program() {
    let out = run_hook(&bash_event("pnpm dev"), true);
    assert!(rewritten_command(&out).contains("--title pnpm "));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test hook_integration`
Expected: FAIL — `filter_value_becomes_title` fails (title is currently always `pnpm`). `without_filter_title_falls_back_to_program` already passes.

- [ ] **Step 3: Replace the title assignment with filter-aware derivation**

In `hooks/route-to-nrun.sh`, replace the single line:

```bash
title="$eprog"
```

with:

```bash
# --- title (priority: --filter > program name; docker image added in Task 5) -
title=""
for ((i = 0; i < ${#eff[@]}; i++)); do
    case "${eff[i]}" in
        --filter | -F) title="${eff[i + 1]:-}"; break ;;
        --filter=*) title="${eff[i]#--filter=}"; break ;;
    esac
done
[ -z "$title" ] && title="$eprog"
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test hook_integration`
Expected: PASS — all tests pass.

- [ ] **Step 5: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 6: Commit**

```bash
git add hooks/route-to-nrun.sh tests/hook_integration.rs
git commit -m "Derive window title from pnpm --filter (#2)"
```

---

## Task 5: Docker rewrite + compose

Add `docker run` and `docker compose up`/`docker-compose up` to the allowlist. For `docker run`: drop `-d`/`--detach`, ensure `--rm` and `--name` (reuse the user's or derive from the image basename), pass the name to `nrun --docker-name`, and title from the image basename. For compose: drop `-d`/`--detach`, title `compose`. Docker options are re-emitted **before** the image so the rewrite stays valid.

**Files:**
- Modify: `hooks/route-to-nrun.sh`
- Modify: `tests/hook_integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/hook_integration.rs`:

```rust
#[test]
fn docker_run_with_explicit_name() {
    let out = run_hook(&bash_event("docker run -d --name pg postgres:16"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title postgres "), "got: {cmd}");
    assert!(cmd.contains("--docker-name pg "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("docker run"), "script: {body}");
    assert!(body.contains("--name pg"), "script: {body}");
    assert!(body.contains("--rm"), "script: {body}");
    assert!(body.contains("postgres:16"), "script: {body}");
    assert!(!body.contains(" -d "), "script must drop -d: {body}");
    assert!(!body.contains("--detach"), "script must drop --detach: {body}");
}

#[test]
fn docker_run_derives_name_from_image() {
    let out = run_hook(&bash_event("docker run -d nginx"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title nginx "), "got: {cmd}");
    assert!(cmd.contains("--docker-name nginx "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("--name nginx"), "script: {body}");
    assert!(body.contains("--rm"), "script: {body}");
}

#[test]
fn docker_run_options_precede_image() {
    let out = run_hook(
        &bash_event("docker run -d -p 5432:5432 -e PASS=x postgres:16"),
        true,
    );
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--docker-name postgres "), "got: {cmd}");
    let body = script_body(&cmd);
    // every injected/kept option must appear before the image token
    let img = body.find("postgres:16").expect("image present");
    for opt in ["-p 5432:5432", "-e PASS=x", "--name postgres", "--rm"] {
        let at = body.find(opt).unwrap_or_else(|| panic!("missing {opt}: {body}"));
        assert!(at < img, "{opt:?} must precede image: {body}");
    }
}

#[test]
fn docker_compose_up_drops_detach_and_titles_compose() {
    for (input, prog) in [
        ("docker compose up -d", "docker compose up"),
        ("docker-compose up -d", "docker-compose up"),
    ] {
        let out = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(cmd.contains("--title compose "), "input {input:?} -> {cmd}");
        assert!(!cmd.contains("--docker-name"), "compose has no docker-name: {cmd}");
        let body = script_body(&cmd);
        assert!(body.contains(&format!("exec {prog}")), "script: {body}");
        assert!(!body.contains(" -d"), "script must drop -d: {body}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test hook_integration`
Expected: FAIL — docker/compose commands currently pass through (not in the allowlist), so `rewritten_command` panics with "expected a rewrite".

- [ ] **Step 3: Add docker/compose to the allowlist**

In `hooks/route-to-nrun.sh`, replace the allowlist block:

```bash
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
```

with (adds the two docker flags and the `docker`/`docker-compose` cases):

```bash
matched=0
is_docker_run=0
is_compose=0
case "$eprog" in
    pnpm) contains dev "${eff[@]:1}" && matched=1 ;;
    npm) contains run "${eff[@]:1}" && contains dev "${eff[@]:1}" && matched=1 ;;
    yarn) contains dev "${eff[@]:1}" && matched=1 ;;
    next) contains dev "${eff[@]:1}" && matched=1 ;;
    vite) matched=1 ;;
    make | cmake) matched=1 ;;
    configure | ./configure) matched=1 ;;
    docker)
        if [ "${eff[1]:-}" = "run" ]; then
            matched=1
            is_docker_run=1
        elif [ "${eff[1]:-}" = "compose" ] && [ "${eff[2]:-}" = "up" ]; then
            matched=1
            is_compose=1
        fi
        ;;
    docker-compose) [ "${eff[1]:-}" = "up" ] && {
        matched=1
        is_compose=1
    } ;;
esac
[ "$matched" = "1" ] || passthrough
```

- [ ] **Step 4: Add the docker/compose rewrite block**

In `hooks/route-to-nrun.sh`, **replace** the single line that sits between the title-derivation block and the `# --- write the temp script` block:

```bash
effective_cmd="$cmd"
```

with the following block (it re-establishes the non-docker default of `effective_cmd` at its top, then overrides it for docker/compose):

```bash
docker_name=""
effective_cmd="$cmd"

if [ "$is_docker_run" = "1" ]; then
    # flags whose argument must be skipped when hunting for the image token
    valflags=" -p --publish -e --env --env-file -v --volume --mount --name -w --workdir --network --net -u --user --entrypoint -l --label -h --hostname --add-host --device --restart --expose --link -m --memory --cpus --gpus --platform --pull "
    args=("${eff[@]:2}")
    opts=()
    cmd_tail=()
    image=""
    has_name=""
    name_val=""
    seen_rm=0
    skip_next=0
    for ((j = 0; j < ${#args[@]}; j++)); do
        a="${args[j]}"
        if [ "$skip_next" = "1" ]; then
            skip_next=0
            opts+=("$a")
            continue
        fi
        if [ -n "$image" ]; then
            cmd_tail+=("$a")
            continue
        fi
        case "$a" in
            -d | --detach) ;;
            --rm)
                seen_rm=1
                opts+=("$a")
                ;;
            --name)
                has_name=1
                name_val="${args[j + 1]:-}"
                opts+=("$a")
                skip_next=1
                ;;
            --name=*)
                has_name=1
                name_val="${a#--name=}"
                opts+=("$a")
                ;;
            -*)
                opts+=("$a")
                [[ " $valflags " == *" $a "* ]] && skip_next=1
                ;;
            *) image="$a" ;;
        esac
    done
    img_base="${image##*/}"
    img_base="${img_base%%:*}"
    img_base="${img_base%%@*}"
    if [ -n "$has_name" ]; then
        docker_name="$name_val"
    else
        docker_name="${img_base:-docker}"
    fi
    rebuilt=("docker" "run")
    [ "${#opts[@]}" -gt 0 ] && rebuilt+=("${opts[@]}")
    [ -z "$has_name" ] && rebuilt+=("--name" "$docker_name")
    [ "$seen_rm" = "0" ] && rebuilt+=("--rm")
    rebuilt+=("$image")
    [ "${#cmd_tail[@]}" -gt 0 ] && rebuilt+=("${cmd_tail[@]}")
    effective_cmd="${rebuilt[*]}"
    [ -z "$title" ] || [ "$title" = "docker" ] && title="${img_base:-docker}"
elif [ "$is_compose" = "1" ]; then
    rebuilt=()
    for t in "${eff[@]}"; do
        case "$t" in
            -d | --detach) ;;
            *) rebuilt+=("$t") ;;
        esac
    done
    effective_cmd="${rebuilt[*]}"
    title="compose"
fi
```

- [ ] **Step 5: Pass `--docker-name` through to nrun**

In `hooks/route-to-nrun.sh`, replace the nrun-command line:

```bash
nrun_cmd="nrun --title $(printf '%q' "$title") $(printf '%q' "$script")"
```

with:

```bash
nrun_cmd="nrun --title $(printf '%q' "$title")"
[ -n "$docker_name" ] && nrun_cmd+=" --docker-name $(printf '%q' "$docker_name")"
nrun_cmd+=" $(printf '%q' "$script")"
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test hook_integration`
Expected: PASS — all tests pass.

- [ ] **Step 7: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 8: Commit**

```bash
git add hooks/route-to-nrun.sh tests/hook_integration.rs
git commit -m "Add docker run/compose rewrite + --docker-name routing (#2)"
```

---

## Task 6: Documentation + final verification

Document the opt-in wiring and confirm the full gate.

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the hook in the README**

Append to `README.md`:

````markdown
## nrun PreToolUse hook

`hooks/route-to-nrun.sh` is a Claude Code `PreToolUse` hook that transparently
routes allowlisted long-lived commands (dev servers, docker, build tools)
through `nrun`, so each runs in a dedicated tmux window while Claude still reads
its output. Unrecognized commands run unchanged. The hook is **fail-open**: any
error, unrecognized input, or missing `nrun` passes the command through verbatim.

### Requirements

- `nrun` on `PATH`: `cargo install --path .`
- `jq` on `PATH`.

### Enable it (opt-in)

Add to `~/.claude/settings.json` (use the absolute path to this checkout):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "/ABSOLUTE/PATH/TO/tmux-claude/main/hooks/route-to-nrun.sh" }
        ]
      }
    ]
  }
}
```

### Allowlist

`pnpm dev`, `npm run dev`, `yarn dev`, `next dev`, `vite` (and `npx next dev` /
`npx vite`), `docker run`, `docker compose up` / `docker-compose up`, `make`,
`cmake`, `configure`. A leading `cd <dir> &&` and leading `VAR=value` env
assignments are folded into the run; commands with pipes, redirects, `;`,
additional `&&`, subshells, or command substitution pass through unchanged.
````

- [ ] **Step 2: Run the full commit gate**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo build --release`
Expected: all succeed.

- [ ] **Step 3: Manual smoke test (optional, requires tmux + nrun installed)**

```bash
cargo install --path .
echo '{"tool_name":"Bash","tool_input":{"command":"pnpm dev","description":"x","timeout":120000,"run_in_background":false}}' \
  | hooks/route-to-nrun.sh | jq .
```
Expected: JSON with `hookSpecificOutput.updatedInput.command` = `nrun --title pnpm …`, `run_in_background: true`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "Document nrun PreToolUse hook wiring + allowlist (#2)"
```

---

## Self-review notes

- **Spec coverage:** allowlist (Tasks 2, 5), uniform `run_in_background: true` (Task 2), `updatedInput` without `permissionDecision` + field preservation (Task 2), conservative match / compound disqualifier (Task 3), cd/env folding (Task 3), title derivation filter→image→program (Tasks 4, 5), docker rewrite rules (Task 5), recursion guard + nrun-absent + fail-open (Tasks 1, 2), Rust-driven tests under `cargo test` (all tasks), documented wiring (Task 6). SessionEnd sweep is intentionally out of scope (issue #3).
- **Known best-effort limits (documented behavior, not bugs):** docker image detection uses a curated value-flag skip-list; an unusual value-taking flag could mis-identify the image (worst case: a less-pretty title/name — teardown still works via the injected `--name`). Two unnamed `docker run`s of the same image derive the same `--name` (collision) — acceptable for v1; users typically pass `--name`. If docker parsing proves fragile in practice, the fallback is porting the hook to Rust (reusing nrun's arg-handling) per the spec's language note.
- **Type/name consistency:** `passthrough`, `contains`, `eff`/`eprog`, `effective_cmd`, `cd_dir`, `env_assignments`, `docker_name`, `is_docker_run`, `is_compose` are defined once and used consistently. Test helpers `run_hook`, `bash_event`, `updated_input`, `rewritten_command`, `script_body` are introduced in Task 1 and reused unchanged.
