use zeroize::Zeroizing;

/// Resolve the CLI bearer token, token-only (Istio-style pre-shared token).
///
/// Precedence (highest first):
/// 1. `flag` — the `--token` command-line value (if supplied).
/// 2. `EDGION_TOKEN` environment variable.
/// 3. The cached token file at `~/.edgion/token`.
///
/// Empty values are treated as absent at every level. Returns the token wrapped
/// in `Zeroizing` so its buffer is wiped on drop. There is no username/password
/// or login exchange; the controller authenticates requests by a pre-shared
/// bearer token only.
pub fn resolve_token(flag: Option<&str>) -> Option<Zeroizing<String>> {
    // 1. --token flag
    if let Some(t) = flag {
        if !t.is_empty() {
            return Some(Zeroizing::new(t.to_string()));
        }
    }

    // 2. EDGION_TOKEN env var
    if let Ok(t) = std::env::var("EDGION_TOKEN") {
        if !t.is_empty() {
            return Some(Zeroizing::new(t));
        }
    }

    // 3. ~/.edgion/token file
    crate::cli::token::read_token()
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    // Process-global lock serializing `EDGION_TOKEN` env manipulation across these
    // tests. CLI tests run in their own test binary (separate process), so a
    // per-crate lock is sufficient.
    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn flag_token_wins() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("EDGION_TOKEN", "edg_env");
        let resolved = resolve_token(Some("edg_flag"));
        assert_eq!(resolved.as_deref().map(|s| s.as_str()), Some("edg_flag"));
        std::env::remove_var("EDGION_TOKEN");
    }

    #[test]
    fn env_token_used_when_flag_absent() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("EDGION_TOKEN", "edg_env");
        let resolved = resolve_token(None);
        assert_eq!(resolved.as_deref().map(|s| s.as_str()), Some("edg_env"));
        std::env::remove_var("EDGION_TOKEN");
    }

    #[test]
    fn empty_flag_falls_through_to_env() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("EDGION_TOKEN", "edg_env");
        let resolved = resolve_token(Some(""));
        assert_eq!(resolved.as_deref().map(|s| s.as_str()), Some("edg_env"));
        std::env::remove_var("EDGION_TOKEN");
    }

    #[test]
    fn empty_env_token_treated_as_absent() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("EDGION_TOKEN", "");
        // With no flag and an empty env var, resolution falls through to the file.
        // We can't assume a token file exists on the test host, so we only assert
        // the env var did not leak through as an empty token.
        let resolved = resolve_token(None);
        assert_ne!(resolved.as_deref().map(|s| s.as_str()), Some(""));
        std::env::remove_var("EDGION_TOKEN");
    }
}
