// Apply / delete helpers — reuse the ctl-cli internal implementation directly to stay in sync with edgion-cli.
//
// API reference (cross-checked with edgion-cli src/cli/{client,commands}.rs):
//   EdgionClient::new(target: TargetType, server: Option<String>, socket: Option<PathBuf>)
//                     -> Result<Self>
//   apply(client, file_path: &str, dry_run: bool) -> Result<()>
//   delete(client, kind: Option<&str>, name: Option<&str>, namespace: Option<&str>,
//          file: Option<&str>) -> Result<()>
//
// Token-only model: `EdgionClient::new` builds a token-LESS client; the bearer is
// attached by `with_static_bearer` (this is what the edgion-cli dispatch does after
// `auto_auth::resolve_token`). The harness exports EDGION_TOKEN to the same value as
// the controller's EDGION_ADMIN_TOKEN, so resolving it here lets dynamic apply/delete
// authenticate. Without this the requests are anonymous and the controller returns 401.

use anyhow::{Context, Result};
use std::path::Path;

use edgion_cli::cli::client::EdgionClient;
use edgion_cli::cli::commands::apply::apply;
use edgion_cli::cli::commands::delete::delete;
use edgion_cli::cli::TargetType;

/// Build an EdgionClient targeting the Controller (ConfCenter), with the admin
/// bearer token resolved the same way edgion-cli does (--token / EDGION_TOKEN /
/// ~/.edgion/token).
pub fn build_client(controller_url: &str) -> Result<EdgionClient> {
    match edgion_cli::cli::auto_auth::resolve_token(None) {
        Some(token) => EdgionClient::with_static_bearer(
            TargetType::Center,
            Some(controller_url.to_string()),
            None,
            token.as_str(),
        )
        .context("Failed to build EdgionClient (bearer) for scenario"),
        None => EdgionClient::new(TargetType::Center, Some(controller_url.to_string()), None)
            .context("Failed to build EdgionClient for scenario"),
    }
}

/// Apply a file or directory (directories are walked for *.yaml / *.yml).
///
/// **NOTE:** When `path` is a directory, the underlying `apply` command only logs per-file failures
/// and continues, returning `Ok(())` overall.
/// Therefore individual YAML failures in directory mode do not propagate as errors; after apply,
/// scenarios should verify gateway state explicitly via `wait_phase` (retrying business assertions)
/// or a phase assertion.
pub async fn apply_path(client: &EdgionClient, path: &Path) -> Result<()> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path: {:?}", path))?;
    apply(client, path_str, false)
        .await
        .with_context(|| format!("apply failed for {}", path.display()))
}

/// Delete resources listed in a file. Each line uses the format `Kind/Namespace/Name`;
/// lines starting with `#` and empty lines are ignored. For cluster-scoped resources
/// (e.g. `EdgionGatewayConfig`, `GatewayClass`) leave the Namespace segment empty:
/// `Kind//Name` — an empty namespace is passed through as `None` so the delete targets the
/// cluster-scoped route rather than the namespaced one.
pub async fn delete_list(client: &EdgionClient, list_file: &Path) -> Result<()> {
    let content = tokio::fs::read_to_string(list_file)
        .await
        .with_context(|| format!("Failed to read delete list: {}", list_file.display()))?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() != 3 {
            anyhow::bail!("Invalid delete list line (expected Kind/Namespace/Name): {}", line);
        }
        let (kind, namespace, name) = (parts[0], parts[1], parts[2]);
        // Empty namespace segment → cluster-scoped delete.
        let namespace = if namespace.is_empty() { None } else { Some(namespace) };
        delete(client, Some(kind), Some(name), namespace, None)
            .await
            .with_context(|| format!("Failed to delete {}/{}/{}", kind, namespace.unwrap_or(""), name))?;
    }
    Ok(())
}
