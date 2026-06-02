// Apply / delete helpers — reuse the ctl-cli internal implementation directly to stay in sync with edgion-cli.
//
// API reference (cross-checked with src/core/ctl/cli/{client,commands}.rs):
//   EdgionClient::new(target: TargetType, server: Option<String>, socket: Option<PathBuf>,
//                     controller_config: Option<String>) -> Result<Self>
//   apply(client, file_path: &str, dry_run: bool) -> Result<()>
//   delete(client, kind: Option<&str>, name: Option<&str>, namespace: Option<&str>,
//          file: Option<&str>) -> Result<()>
//
// The admin token is read automatically by EdgionClient::new from ~/.edgion/token (see client.rs).

use anyhow::{Context, Result};
use std::path::Path;

use edgion::core::ctl::cli::client::EdgionClient;
use edgion::core::ctl::cli::commands::apply::apply;
use edgion::core::ctl::cli::commands::delete::delete;
use edgion::core::ctl::cli::TargetType;

/// Build an EdgionClient targeting the Controller (ConfCenter).
pub fn build_client(controller_url: &str) -> Result<EdgionClient> {
    EdgionClient::new(TargetType::Center, Some(controller_url.to_string()), None, None)
        .context("Failed to build EdgionClient for scenario")
}

/// Apply a file or directory (directories are walked for *.yaml / *.yml).
///
/// **NOTE:** When `path` is a directory, `apply_directory` only print_error's on per-file failures
/// and continues, returning `Ok(())` overall (see `src/core/ctl/cli/commands/apply.rs:38`).
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
