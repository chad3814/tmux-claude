//! Black-box integration tests for the PreToolUse hook `hooks/route-to-nrun.sh`.
//!
//! Each test runs the hook in an isolated PATH containing only symlinks to the
//! tools the hook needs (plus, optionally, a dummy `nrun`), feeds a JSON event
//! on stdin, and asserts on the JSON (or empty) stdout.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Owns an isolated test root directory; removes it on drop.
struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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
fn make_env(with_nrun: bool) -> TestRoot {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("nrun-hooktest-{}-{}", std::process::id(), n));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    assert!(
        which("jq").is_some(),
        "jq must be installed for hook integration tests"
    );
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
    TestRoot(root)
}

/// Run the hook with `stdin` in an isolated PATH. Returns stdout and the test
/// root guard (which cleans up the temp dir when dropped). Asserts exit 0.
fn run_hook(stdin: &str, with_nrun: bool) -> (String, TestRoot) {
    let root = make_env(with_nrun);
    let bin = root.0.join("bin");
    let mut child = Command::new(hook_path())
        .env("PATH", &bin)
        .env("TMPDIR", &root.0)
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
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    (stdout, root)
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
    let (out, _root) = run_hook(&event, true);
    assert!(out.trim().is_empty());
}

#[test]
fn malformed_json_passes_through() {
    let (out, _root) = run_hook("{ this is not json", true);
    assert!(out.trim().is_empty());
}

#[test]
fn empty_command_passes_through() {
    let (out, _root) = run_hook(&bash_event(""), true);
    assert!(out.trim().is_empty());
}

#[test]
fn nrun_absent_passes_through() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), false);
    assert!(out.trim().is_empty());
}

#[test]
fn pnpm_dev_is_rewritten_to_nrun() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.starts_with("nrun --title pnpm "), "got: {cmd}");
    let ui = updated_input(&out).unwrap();
    assert_eq!(ui["run_in_background"], serde_json::json!(true));
}

#[test]
fn rewrite_preserves_original_tool_input_fields() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
    let ui = updated_input(&out).unwrap();
    assert_eq!(ui["description"], serde_json::json!("d"));
    assert_eq!(ui["timeout"], serde_json::json!(120000));
}

#[test]
fn generated_script_execs_the_command() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
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
        let (out, _root) = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(
            cmd.contains(&format!("--title {title} ")),
            "input {input:?} -> {cmd}"
        );
    }
}

#[test]
fn non_allowlisted_commands_pass_through() {
    for input in [
        "ls",
        "npm test",
        "git status",
        "echo pnpm dev",
        "pnpm",
        "pnpm build",
    ] {
        let (out, _root) = run_hook(&bash_event(input), true);
        assert!(out.trim().is_empty(), "expected pass-through for {input:?}");
    }
}

#[test]
fn recursion_guard_passes_nrun_through() {
    let (out, _root) = run_hook(&bash_event("nrun --title x /tmp/s.sh"), true);
    assert!(out.trim().is_empty());
}

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
        let (out, _root) = run_hook(&bash_event(input), true);
        assert!(out.trim().is_empty(), "expected pass-through for {input:?}");
    }
}

#[test]
fn leading_cd_is_folded_into_script() {
    let (out, _root) = run_hook(&bash_event("cd app && pnpm dev"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title pnpm "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("cd app"), "script was: {body}");
    assert!(body.contains("exec pnpm dev"), "script was: {body}");
}

#[test]
fn leading_env_assignments_are_folded_into_script() {
    let (out, _root) = run_hook(&bash_event("FORCE_COLOR=1 vite"), true);
    let body = script_body(&rewritten_command(&out));
    assert!(body.contains("export FORCE_COLOR=1"), "script was: {body}");
    assert!(body.contains("exec vite"), "script was: {body}");
}

#[test]
fn env_value_with_quote_passes_through() {
    // A quote in an env value can't be folded into the unquoted `export` line,
    // so the whole command must pass through (fail-open), not produce a rewrite.
    let (out, _root) = run_hook(&bash_event("FOO=ba'r vite"), true);
    assert!(out.trim().is_empty(), "got: {out}");
}

#[test]
fn cd_and_env_combined_fold() {
    let (out, _root) = run_hook(&bash_event("cd app && DEBUG=1 pnpm dev"), true);
    let body = script_body(&rewritten_command(&out));
    assert!(body.contains("cd app\n"), "script: {body}");
    assert!(body.contains("export DEBUG=1\n"), "script: {body}");
    assert!(body.contains("exec pnpm dev"), "script: {body}");
}

#[test]
fn filter_value_becomes_title() {
    for (input, title) in [
        ("pnpm --filter web dev", "web"),
        ("pnpm -F app dev", "app"),
        ("pnpm --filter=api dev", "api"),
    ] {
        let (out, _root) = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(
            cmd.contains(&format!("--title {title} ")),
            "input {input:?} -> {cmd}"
        );
    }
}

#[test]
fn without_filter_title_falls_back_to_program() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
    assert!(rewritten_command(&out).contains("--title pnpm "));
}

#[test]
fn docker_run_with_explicit_name() {
    let (out, _root) = run_hook(&bash_event("docker run -d --name pg postgres:16"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title postgres "), "got: {cmd}");
    assert!(cmd.contains("--docker-name pg "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("docker run"), "script: {body}");
    assert!(body.contains("--name pg"), "script: {body}");
    assert!(body.contains("--rm"), "script: {body}");
    assert!(body.contains("postgres:16"), "script: {body}");
    assert!(!body.contains(" -d "), "script must drop -d: {body}");
    assert!(
        !body.contains("--detach"),
        "script must drop --detach: {body}"
    );
}

#[test]
fn docker_run_derives_name_from_image() {
    let (out, _root) = run_hook(&bash_event("docker run -d nginx"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title nginx "), "got: {cmd}");
    assert!(cmd.contains("--docker-name nginx "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("--name nginx"), "script: {body}");
    assert!(body.contains("--rm"), "script: {body}");
}

#[test]
fn docker_run_options_precede_image() {
    let (out, _root) = run_hook(
        &bash_event("docker run -d -p 5432:5432 -e PASS=x postgres:16"),
        true,
    );
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--docker-name postgres "), "got: {cmd}");
    let body = script_body(&cmd);
    // every injected/kept option must appear before the image token
    let img = body.find("postgres:16").expect("image present");
    for opt in ["-p 5432:5432", "-e PASS=x", "--name postgres", "--rm"] {
        let at = body
            .find(opt)
            .unwrap_or_else(|| panic!("missing {opt}: {body}"));
        assert!(at < img, "{opt:?} must precede image: {body}");
    }
}

#[test]
fn docker_compose_up_drops_detach_and_titles_compose() {
    for (input, prog) in [
        ("docker compose up -d", "docker compose up"),
        ("docker-compose up -d", "docker-compose up"),
    ] {
        let (out, _root) = run_hook(&bash_event(input), true);
        let cmd = rewritten_command(&out);
        assert!(cmd.contains("--title compose "), "input {input:?} -> {cmd}");
        assert!(
            !cmd.contains("--docker-name"),
            "compose has no docker-name: {cmd}"
        );
        let body = script_body(&cmd);
        assert!(body.contains(&format!("exec {prog}")), "script: {body}");
        assert!(!body.contains(" -d"), "script must drop -d: {body}");
    }
}

#[test]
fn docker_run_rm_not_duplicated_when_present() {
    let (out, _root) = run_hook(&bash_event("docker run --rm -d nginx"), true);
    let body = script_body(&rewritten_command(&out));
    assert_eq!(body.matches("--rm").count(), 1, "exactly one --rm: {body}");
    assert!(!body.contains(" -d "), "must drop -d: {body}");
}

#[test]
fn docker_run_name_equals_form() {
    let (out, _root) = run_hook(&bash_event("docker run -d --name=pg postgres:16"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--docker-name pg "), "got: {cmd}");
    assert!(cmd.contains("--title postgres "), "got: {cmd}");
    let body = script_body(&cmd);
    assert!(body.contains("--name=pg"), "script: {body}");
}

#[test]
fn docker_run_container_command_tail_follows_image() {
    let (out, _root) = run_hook(&bash_event("docker run -d alpine echo hi"), true);
    let body = script_body(&rewritten_command(&out));
    // the container command (echo hi) must come AFTER the image token
    assert!(body.contains("alpine echo hi"), "tail after image: {body}");
    assert!(body.contains("--name alpine"), "script: {body}");
}

#[test]
fn docker_run_keeps_it_and_finds_image() {
    let (out, _root) = run_hook(&bash_event("docker run -it ubuntu"), true);
    let cmd = rewritten_command(&out);
    assert!(cmd.contains("--title ubuntu "), "got: {cmd}");
    assert!(cmd.contains("--docker-name ubuntu "), "got: {cmd}");
    let body = script_body(&cmd);
    // -it must be kept and must NOT have swallowed the image
    let it = body.find("-it").expect("-it kept");
    let img = body.rfind("ubuntu").expect("image present");
    assert!(it < img, "-it before image: {body}");
    assert!(body.contains("--rm"), "script: {body}");
}
