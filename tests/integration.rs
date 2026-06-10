//! End-to-end tests that drive a real tmux server.
//!
//! These are skipped when not running inside tmux (e.g. CI without a tmux
//! session). When inside tmux they create and tear down windows named
//! `nrun-it-*` in the current session.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_nrun")
}

fn write_script(tag: &str, body: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("nrun-it-{}-{}.sh", tag, std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    writeln!(f, "#!/bin/bash").unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

fn window_exists(name: &str) -> bool {
    let out = Command::new("tmux")
        .args(["list-windows", "-a", "-F", "#{window_name}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l == name)
}

fn wait_until(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    pred()
}

#[test]
fn captures_output_and_propagates_exit_code() {
    if !in_tmux() {
        eprintln!("skipping captures_output_and_propagates_exit_code: not inside tmux");
        return;
    }
    let script = write_script("cap", "printf 'NRUN_HELLO\\n'\nexit 7\n");
    let log = std::env::temp_dir().join(format!("nrun-it-cap-{}.log", std::process::id()));

    let out = Command::new(bin())
        .args([
            "--title",
            "nrun-it-cap",
            "--log",
            log.to_str().unwrap(),
            script.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("NRUN_HELLO"),
        "expected captured output, got stdout={stdout:?} stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(7), "exit code should propagate");
    assert!(
        !window_exists("nrun-it-cap"),
        "window should be cleaned up after exit"
    );

    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&log);
}

#[test]
fn terminates_and_cleans_up_on_sigterm() {
    if !in_tmux() {
        eprintln!("skipping terminates_and_cleans_up_on_sigterm: not inside tmux");
        return;
    }
    let script = write_script("long", "printf 'STARTED\\n'\nsleep 600\n");
    let log = std::env::temp_dir().join(format!("nrun-it-long-{}.log", std::process::id()));

    let mut child = Command::new(bin())
        .args([
            "--title",
            "nrun-it-long",
            "--log",
            log.to_str().unwrap(),
            script.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    assert!(
        wait_until(|| window_exists("nrun-it-long"), Duration::from_secs(5)),
        "window should appear"
    );

    // Ask nrun to shut down.
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();

    let exited = wait_until(
        || child.try_wait().ok().flatten().is_some(),
        Duration::from_secs(5),
    );
    assert!(exited, "nrun should exit promptly after SIGTERM");

    assert!(
        wait_until(|| !window_exists("nrun-it-long"), Duration::from_secs(5)),
        "window (and its sleep) should be cleaned up"
    );

    let _ = child.wait();
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&log);
}
