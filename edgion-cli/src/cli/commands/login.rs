use anyhow::{Context, Result};
use edgion_common::link_sys_core::http_client::{OutboundClientConfig, OutboundRedirect};
use std::path::Path;
use std::time::Duration;

use crate::cli::client::{diagnostic_origin, error_chain, login_probe_trust, ServerTrust};
use crate::cli::token::save_token;

/// Execute the `login` command.
///
/// Authentication is a pre-shared bearer token only — there is no
/// username/password exchange. This command simply persists the supplied token
/// to `~/.edgion/token` so subsequent invocations pick it up automatically
/// (file is the lowest-precedence source, below `--token` and `EDGION_TOKEN`).
///
/// As a friendly convenience it first probes `GET /api/v1/auth/status` to warn
/// when the controller is unreachable or its auth subsystem is not ready. The
/// probe never blocks persisting: the token is saved regardless, since the token
/// is validated lazily by the first real API call (401 = unknown, 403 = denied).
/// The probe uses `ca_cert` (`--ca-cert`); an unusable file skips the probe with a
/// warning.
pub async fn login(base_url: &str, token: &str, ca_cert: Option<&Path>) -> Result<()> {
    if token.is_empty() {
        anyhow::bail!("Token cannot be empty");
    }

    // Optional reachability/readiness probe — best-effort, never fatal.
    if let Some(trust) = login_probe_trust(ca_cert) {
        probe_status(base_url, &trust).await;
    }

    // Persist the token to ~/.edgion/token (0600).
    save_token(token).context("Failed to save token")?;

    println!("Token saved to ~/.edgion/token.");
    Ok(())
}

/// Best-effort `GET /api/v1/auth/status` probe. Prints a friendly hint when the
/// controller is unreachable or its auth subsystem is not ready, but never fails
/// the command — the token is persisted either way.
async fn probe_status(base_url: &str, trust: &ServerTrust) {
    let url = format!("{}/api/v1/auth/status", base_url);
    // Built through the canonical factory (design rule D.1). `disable_proxy` is
    // left false on purpose: this is an operator-run CLI that must be able to
    // reach a controller through a corporate `HTTP(S)_PROXY`.
    let Ok(client) = trust.build_client(OutboundClientConfig {
        total_timeout: Some(Duration::from_secs(5)),
        redirect: OutboundRedirect::Deny,
        disable_proxy: false,
        ..Default::default()
    }) else {
        return;
    };

    let origin = diagnostic_origin(&url);
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => { /* reachable and ready enough */ }
        Ok(resp) => {
            eprintln!(
                "warning: {} returned HTTP {} (token saved anyway).",
                origin,
                resp.status()
            );
        }
        Err(error) => {
            eprintln!(
                "warning: could not reach {}: {} (token saved anyway).",
                origin,
                error_chain(&error.without_url())
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/cli/commands/login_test.rs"]
mod tests;
