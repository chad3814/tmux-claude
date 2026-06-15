//! Orchestration: create the tmux window, start capture, run the command,
//! stream the log to stdout, and own teardown on exit or signal.

use crate::cli::Config;
use crate::tmux::{self, PaneStatus};
use crate::{docker, tail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

/// How often the supervisor wakes to pump the log and poll pane liveness.
const POLL_INTERVAL: Duration = Duration::from_millis(150);
/// Grace period after the process dies / is signalled before the final drain,
/// to let `pipe-pane`'s `cat` flush trailing output.
const FLUSH_GRACE: Duration = Duration::from_millis(200);

/// Run the configured command under supervision. Returns the process exit code
/// to propagate (128 + signo when terminated by a forwarded signal).
pub fn run(cfg: &Config) -> io::Result<i32> {
    // Not inside tmux: degrade gracefully by becoming the command directly.
    if std::env::var_os("TMUX").is_none() {
        return Err(Command::new("bash").arg(&cfg.script).exec());
    }

    let pid = std::process::id();
    let log = cfg.log_path(pid);
    let owns_log = cfg.log.is_none();

    // Pre-create the capture log so the tailer can open it right away.
    File::create(&log)?;

    // 1. Create an idle window (quiet placeholder) and learn its pane id.
    let pane_id = tmux::parse_pane_id(&tmux_capture(&tmux::new_window_argv(&cfg.title))?)
        .ok_or_else(|| io::Error::other("tmux did not return a pane id"))?;

    // 2. Keep the pane after its process exits so we can read the exit status.
    let _ = tmux_ok(&tmux::set_remain_on_exit_argv(&pane_id));

    // 3. Start capture BEFORE the real command runs (no lost early output).
    tmux_run(&tmux::pipe_pane_argv(&pane_id, &log))?;

    // 4. Swap the placeholder for the real command.
    tmux_run(&tmux::respawn_pane_argv(&pane_id, &cfg.script))?;

    // 5. Learn the command's pid (== pane pid thanks to the script's `exec`).
    let pane_pid = tmux_capture(&tmux::display_pane_pid_argv(&pane_id))
        .ok()
        .and_then(|s| tmux::parse_pid(&s));

    // Record which signal (if any) asked us to stop.
    let got = Arc::new(AtomicUsize::new(0));
    for sig in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register_usize(sig, Arc::clone(&got), sig as usize)?;
    }

    let mut reader = File::open(&log)?;
    let stdout = io::stdout();
    let docker_name = cfg.docker_name.as_deref();

    let code = loop {
        pump(&mut reader, &stdout);

        let sig = got.load(Ordering::Relaxed);
        if sig != 0 {
            sleep(FLUSH_GRACE);
            pump(&mut reader, &stdout);
            teardown(&pane_id, pane_pid, docker_name, &log, owns_log, true);
            break 128 + sig as i32;
        }

        match query_pane_status(&pane_id) {
            Some(PaneStatus::Alive) => {}
            Some(PaneStatus::Dead(status)) => {
                sleep(FLUSH_GRACE);
                pump(&mut reader, &stdout);
                teardown(&pane_id, pane_pid, docker_name, &log, owns_log, false);
                break status.unwrap_or(0);
            }
            None => {
                // Window vanished (closed elsewhere) — nothing left to manage.
                teardown(&pane_id, pane_pid, docker_name, &log, owns_log, false);
                break 0;
            }
        }

        sleep(POLL_INTERVAL);
    };

    Ok(code)
}

/// Relay any newly-captured bytes to our own stdout (best effort).
fn pump(reader: &mut File, stdout: &io::Stdout) {
    let mut lock = stdout.lock();
    let _ = tail::pump_available(reader, &mut lock);
    let _ = lock.flush();
}

/// Tear down the window and any associated resources. Idempotent enough to be
/// called once on the way out of the supervision loop.
fn teardown(
    pane_id: &str,
    pane_pid: Option<i32>,
    docker_name: Option<&str>,
    log: &Path,
    owns_log: bool,
    forward_term: bool,
) {
    if forward_term {
        if let Some(p) = pane_pid {
            let _ = kill(Pid::from_raw(p), Signal::SIGTERM);
        }
        sleep(Duration::from_millis(300));
    }
    if let Some(name) = docker_name {
        let _ = Command::new("docker")
            .args(docker::docker_stop_argv(name))
            .output();
    }
    let _ = tmux_ok(&tmux::kill_window_argv(pane_id));
    if owns_log {
        let _ = std::fs::remove_file(log);
    }
}

/// Query pane liveness; `None` if the pane no longer exists.
fn query_pane_status(pane_id: &str) -> Option<PaneStatus> {
    let out = Command::new("tmux")
        .args(tmux::display_status_argv(pane_id))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(tmux::parse_pane_status(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Run a tmux command, capturing stdout; error if tmux exits non-zero.
fn tmux_capture(argv: &[String]) -> io::Result<String> {
    let out = Command::new("tmux").args(argv).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "tmux {} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a tmux command, erroring if it exits non-zero.
fn tmux_run(argv: &[String]) -> io::Result<()> {
    tmux_capture(argv).map(|_| ())
}

/// Run a tmux command best-effort, returning whether it succeeded.
fn tmux_ok(argv: &[String]) -> bool {
    Command::new("tmux")
        .args(argv)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
