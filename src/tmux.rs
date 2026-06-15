//! Pure builders for `tmux` argument vectors and parsers for its output.
//!
//! Everything here is side-effect free so it can be unit tested without a
//! running tmux server. The argv vectors returned do **not** include the
//! leading `tmux` program name.

use std::path::Path;

/// Liveness of a tmux pane, derived from `#{pane_dead}`/`#{pane_dead_status}`.
#[derive(Debug, PartialEq, Eq)]
pub enum PaneStatus {
    /// The pane's process is still running.
    Alive,
    /// The pane's process has exited; the wait status if tmux reported one.
    Dead(Option<i32>),
}

/// Quote a string for safe inclusion in a `/bin/sh -c` command line.
pub fn sh_single_quote(s: &str) -> String {
    // Close the quote, emit an escaped literal quote, reopen: ' -> '\''
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

/// A quiet, long-lived placeholder the new window runs until `respawn-pane`
/// swaps in the real command. Keeping the pane idle (rather than launching the
/// command immediately) lets us enable `pipe-pane` first, so no early output is
/// lost.
pub const PLACEHOLDER_COMMAND: &str = "exec sleep 2147483647";

/// The command tmux runs in the pane: exec the script so the pane pid becomes
/// the real process (see the script's trailing `exec`).
pub fn pane_command(script: &Path) -> String {
    format!("exec bash {}", sh_single_quote(&script.to_string_lossy()))
}

/// `new-window` argv that prints the new pane id and runs a quiet placeholder
/// detached, ready for `pipe-pane` then `respawn-pane`.
pub fn new_window_argv(title: &str) -> Vec<String> {
    vec![
        "new-window".into(),
        "-d".into(),
        "-P".into(),
        "-F".into(),
        "#{pane_id}".into(),
        "-n".into(),
        title.into(),
        PLACEHOLDER_COMMAND.into(),
    ]
}

/// `respawn-pane` argv that replaces the placeholder with the real command,
/// after capture is already live.
pub fn respawn_pane_argv(pane_id: &str, script: &Path) -> Vec<String> {
    vec![
        "respawn-pane".into(),
        "-k".into(),
        "-t".into(),
        pane_id.into(),
        pane_command(script),
    ]
}

/// `set-option` argv to keep the pane in a dead state after the process exits,
/// so its exit status can be read.
pub fn set_remain_on_exit_argv(pane_id: &str) -> Vec<String> {
    vec![
        "set-option".into(),
        "-p".into(),
        "-t".into(),
        pane_id.into(),
        "remain-on-exit".into(),
        "on".into(),
    ]
}

/// `pipe-pane` argv that appends the pane's output to the capture log.
pub fn pipe_pane_argv(pane_id: &str, log: &Path) -> Vec<String> {
    vec![
        "pipe-pane".into(),
        "-o".into(),
        "-t".into(),
        pane_id.into(),
        format!("cat >> {}", sh_single_quote(&log.to_string_lossy())),
    ]
}

/// `display-message` argv that prints the pane's process id.
pub fn display_pane_pid_argv(pane_id: &str) -> Vec<String> {
    vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        pane_id.into(),
        "#{pane_pid}".into(),
    ]
}

/// `display-message` argv that prints `dead|status` for the pane.
pub fn display_status_argv(pane_id: &str) -> Vec<String> {
    vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        pane_id.into(),
        "#{pane_dead}|#{pane_dead_status}".into(),
    ]
}

/// `kill-window` argv for the pane's window.
pub fn kill_window_argv(pane_id: &str) -> Vec<String> {
    vec!["kill-window".into(), "-t".into(), pane_id.into()]
}

/// Parse a pane id (e.g. `%3`) from `display-message` output.
pub fn parse_pane_id(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a process id from `display-message` output.
pub fn parse_pid(stdout: &str) -> Option<i32> {
    stdout.trim().parse().ok()
}

/// Parse `#{pane_dead}|#{pane_dead_status}` output into a [`PaneStatus`].
pub fn parse_pane_status(stdout: &str) -> PaneStatus {
    let trimmed = stdout.trim_end_matches(['\n', '\r']);
    let (dead, status) = trimmed.split_once('|').unwrap_or((trimmed, ""));
    if dead.trim() == "1" {
        PaneStatus::Dead(status.trim().parse().ok())
    } else {
        PaneStatus::Alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_single_quote_wraps_plain_strings() {
        assert_eq!(sh_single_quote("abc"), "'abc'");
        assert_eq!(sh_single_quote("a b c"), "'a b c'");
    }

    #[test]
    fn sh_single_quote_escapes_embedded_quotes() {
        assert_eq!(sh_single_quote("it's"), r#"'it'\''s'"#);
    }

    #[test]
    fn pane_command_execs_bash_with_quoted_script() {
        assert_eq!(
            pane_command(Path::new("/tmp/run web.sh")),
            "exec bash '/tmp/run web.sh'"
        );
    }

    #[test]
    fn new_window_argv_is_detached_and_prints_pane_id() {
        assert_eq!(
            new_window_argv("web"),
            vec![
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-n",
                "web",
                "exec sleep 2147483647",
            ]
        );
    }

    #[test]
    fn respawn_pane_argv_replaces_with_quoted_command() {
        assert_eq!(
            respawn_pane_argv("%3", Path::new("/r.sh")),
            vec!["respawn-pane", "-k", "-t", "%3", "exec bash '/r.sh'"]
        );
    }

    #[test]
    fn set_remain_on_exit_argv_targets_pane() {
        assert_eq!(
            set_remain_on_exit_argv("%3"),
            vec!["set-option", "-p", "-t", "%3", "remain-on-exit", "on"]
        );
    }

    #[test]
    fn pipe_pane_argv_appends_to_quoted_log() {
        assert_eq!(
            pipe_pane_argv("%3", Path::new("/tmp/x.log")),
            vec!["pipe-pane", "-o", "-t", "%3", "cat >> '/tmp/x.log'"]
        );
    }

    #[test]
    fn display_argv_builders() {
        assert_eq!(
            display_pane_pid_argv("%3"),
            vec!["display-message", "-p", "-t", "%3", "#{pane_pid}"]
        );
        assert_eq!(
            display_status_argv("%3"),
            vec![
                "display-message",
                "-p",
                "-t",
                "%3",
                "#{pane_dead}|#{pane_dead_status}"
            ]
        );
    }

    #[test]
    fn kill_window_argv_targets_pane() {
        assert_eq!(kill_window_argv("%3"), vec!["kill-window", "-t", "%3"]);
    }

    #[test]
    fn parse_pane_id_trims_and_rejects_empty() {
        assert_eq!(parse_pane_id("%3\n"), Some("%3".to_string()));
        assert_eq!(parse_pane_id("  %12  \n"), Some("%12".to_string()));
        assert_eq!(parse_pane_id(""), None);
        assert_eq!(parse_pane_id("   \n"), None);
    }

    #[test]
    fn parse_pid_parses_or_rejects() {
        assert_eq!(parse_pid("12345\n"), Some(12345));
        assert_eq!(parse_pid("  42 "), Some(42));
        assert_eq!(parse_pid("notapid"), None);
        assert_eq!(parse_pid(""), None);
    }

    #[test]
    fn parse_pane_status_alive() {
        assert_eq!(parse_pane_status("0|"), PaneStatus::Alive);
        assert_eq!(parse_pane_status("0|\n"), PaneStatus::Alive);
    }

    #[test]
    fn parse_pane_status_dead_with_status() {
        assert_eq!(parse_pane_status("1|0"), PaneStatus::Dead(Some(0)));
        assert_eq!(parse_pane_status("1|137\n"), PaneStatus::Dead(Some(137)));
    }

    #[test]
    fn parse_pane_status_dead_without_status() {
        assert_eq!(parse_pane_status("1|"), PaneStatus::Dead(None));
    }
}
