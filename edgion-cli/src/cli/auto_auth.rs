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
#[path = "../../tests/unit/cli/auto_auth_test.rs"]
mod tests;
