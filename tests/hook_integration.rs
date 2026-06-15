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
fn bash_command_with_nrun_present_passes_through() {
    let (out, _root) = run_hook(&bash_event("pnpm dev"), true);
    assert!(out.trim().is_empty());
}
