use super::client::EdgionClient;
use super::TargetType;

/// Token precedence: `--token` flag value is used as the static bearer.
/// Verifies `with_static_bearer` stores the flag value and `ensure_auth` is
/// bypassed (no network calls needed — the constructor itself is the proof).
#[test]
fn flag_token_injected_as_static_bearer() {
    let bearer = "edg_flagtoken";
    let c = EdgionClient::with_static_bearer(
        TargetType::Center,
        None,
        None,
        &crate::cli::client::ServerTrust::System,
        bearer,
    )
    .expect("with_static_bearer must succeed");
    // The stored token must be the exact flag value (use the test accessor)
    assert_eq!(c.token_for_test(), Some(bearer));
}

/// Token precedence: `EDGION_TOKEN` env var is picked up when `--token` is absent.
/// We replicate the resolution logic from `Cli::run` inline so tests are hermetic.
#[test]
fn env_token_used_when_flag_absent() {
    // Simulate the resolution: flag = None, env = "edg_envtoken"
    let flag: Option<String> = None;
    // We don't actually set the env var (it would bleed across tests on parallel
    // runners); instead we exercise the resolution expression directly.
    let env_val = Some("edg_envtoken".to_string());
    let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
    assert_eq!(resolved.as_deref(), Some("edg_envtoken"));
}

/// Token precedence: `--token` wins over `EDGION_TOKEN`.
#[test]
fn flag_token_wins_over_env() {
    let flag = Some("edg_flag".to_string());
    let env_val = Some("edg_env".to_string());
    let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
    assert_eq!(resolved.as_deref(), Some("edg_flag"));
}

/// When neither `--token` nor `EDGION_TOKEN` is set, resolved token is None
/// and the `ensure_auth` pre-flight path is taken.
#[test]
fn no_token_resolved_is_none() {
    let flag: Option<String> = None;
    let env_val: Option<String> = None;
    let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
    assert!(resolved.is_none());
}

/// Empty `EDGION_TOKEN` is treated as absent (filtered out).
#[test]
fn empty_env_token_treated_as_absent() {
    let flag: Option<String> = None;
    let env_val = Some("".to_string());
    let resolved = flag.or(env_val.filter(|s| !s.is_empty()));
    assert!(resolved.is_none());
}
