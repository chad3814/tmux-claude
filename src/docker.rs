//! Pure builders for `docker` argument vectors used during teardown.

/// `docker stop <name>` argv (without the leading `docker`).
pub fn docker_stop_argv(name: &str) -> Vec<String> {
    vec!["stop".into(), name.into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_stop_argv_targets_named_container() {
        assert_eq!(docker_stop_argv("pg"), vec!["stop", "pg"]);
    }
}
