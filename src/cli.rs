//! Command-line configuration for nrun.

use clap::Parser;
use std::path::PathBuf;

/// Run a command inside a dedicated tmux window with lifecycle management.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(name = "nrun", version, about)]
pub struct Config {
    /// tmux window title.
    #[arg(short, long)]
    pub title: String,

    /// Path to write captured pane output. Defaults to a temp file.
    #[arg(long)]
    pub log: Option<PathBuf>,

    /// docker container name to `docker stop` during teardown.
    #[arg(long = "docker-name")]
    pub docker_name: Option<String>,

    /// Script file to run inside the tmux window.
    pub script: PathBuf,
}

impl Config {
    /// Resolve the log path, falling back to a per-process temp file.
    pub fn log_path(&self, pid: u32) -> PathBuf {
        match &self.log {
            Some(p) => p.clone(),
            None => default_log_path(&self.title, pid),
        }
    }
}

/// Reduce a window title to characters safe for a filename.
pub fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Default capture-log location for a given title and process id.
pub fn default_log_path(title: &str, pid: u32) -> PathBuf {
    std::env::temp_dir().join(format!("nrun-{}-{}.log", sanitize_title(title), pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_title_and_script() {
        let cfg = Config::try_parse_from(["nrun", "--title", "web", "/tmp/run.sh"]).unwrap();
        assert_eq!(cfg.title, "web");
        assert_eq!(cfg.script, PathBuf::from("/tmp/run.sh"));
        assert_eq!(cfg.log, None);
        assert_eq!(cfg.docker_name, None);
    }

    #[test]
    fn parses_optional_log_and_docker_name() {
        let cfg = Config::try_parse_from([
            "nrun",
            "--title",
            "web",
            "--log",
            "/var/log/web.log",
            "--docker-name",
            "pg",
            "/tmp/run.sh",
        ])
        .unwrap();
        assert_eq!(cfg.log, Some(PathBuf::from("/var/log/web.log")));
        assert_eq!(cfg.docker_name, Some("pg".to_string()));
    }

    #[test]
    fn title_and_script_are_required() {
        assert!(Config::try_parse_from(["nrun"]).is_err());
        assert!(Config::try_parse_from(["nrun", "--title", "web"]).is_err());
    }

    #[test]
    fn sanitize_title_keeps_safe_chars_and_replaces_others() {
        assert_eq!(sanitize_title("web"), "web");
        assert_eq!(sanitize_title("web-app_2.1"), "web-app_2.1");
        assert_eq!(sanitize_title("web app/2"), "web-app-2");
        // dots are preserved (safe in filenames); slashes become dashes
        assert_eq!(sanitize_title("../etc"), "..-etc");
    }

    #[test]
    fn default_log_path_uses_temp_dir_title_and_pid() {
        let p = default_log_path("web app", 4242);
        assert_eq!(p.parent().unwrap(), std::env::temp_dir());
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            "nrun-web-app-4242.log"
        );
    }

    #[test]
    fn log_path_prefers_explicit_value() {
        let cfg =
            Config::try_parse_from(["nrun", "--title", "web", "--log", "/x.log", "/r.sh"]).unwrap();
        assert_eq!(cfg.log_path(1), PathBuf::from("/x.log"));
    }

    #[test]
    fn log_path_falls_back_to_default() {
        let cfg = Config::try_parse_from(["nrun", "--title", "web", "/r.sh"]).unwrap();
        assert_eq!(cfg.log_path(7), default_log_path("web", 7));
    }
}
