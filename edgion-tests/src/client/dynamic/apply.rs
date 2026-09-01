// Apply / delete helpers for dynamic integration scenarios.
//
// API reference (cross-checked with edgion-cli src/cli/{client,commands}.rs):
//   EdgionClient::new(target: TargetType, server: Option<String>, socket: Option<PathBuf>)
//                     -> Result<Self>
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

use edgion_cli::cli::client::{handle_response, EdgionClient};
use edgion_cli::cli::commands::delete::delete;
use edgion_cli::cli::TargetType;
use edgion_resources::resource::meta::extract_resource_metadata;

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

/// Apply a file or the immediate YAML files in a directory.
///
/// Dynamic scenarios use a test-owned apply path so an unconditional Admin API
/// PUT can absorb the narrow Kubernetes status-write conflict without changing
/// the product CLI or Controller contract. Files are sorted and every failure
/// propagates to the scenario.
pub async fn apply_path(client: &EdgionClient, path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("File or directory not found: {}", path.display());
    }

    if path.is_file() {
        return apply_file(client, path).await;
    }

    let mut entries = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("Failed to read apply directory: {}", path.display()))?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let extension = entry_path.extension().and_then(|value| value.to_str());
        if entry_path.is_file() && matches!(extension, Some("yaml") | Some("yml")) {
            files.push(entry_path);
        }
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("No YAML files found in {}", path.display());
    }
    for file in files {
        apply_file(client, &file).await?;
    }
    Ok(())
}

fn split_yaml_docs(content: &str) -> Vec<String> {
    let mut documents = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("---") && trimmed.trim_start_matches('-').trim().is_empty() {
            if !current.trim().is_empty() {
                documents.push(std::mem::take(&mut current));
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        documents.push(current);
    }
    documents
}

async fn apply_file(client: &EdgionClient, file: &Path) -> Result<()> {
    let content = tokio::fs::read_to_string(file)
        .await
        .with_context(|| format!("Failed to read apply file: {}", file.display()))?;
    let documents = split_yaml_docs(&content);
    if documents.is_empty() {
        anyhow::bail!("Empty YAML file: {}", file.display());
    }

    for (index, document) in documents.iter().enumerate() {
        apply_document(client, file, document)
            .await
            .with_context(|| format!("Failed to apply document {} from {}", index + 1, file.display()))?;
    }
    Ok(())
}

async fn apply_document(client: &EdgionClient, file: &Path, content: &str) -> Result<()> {
    let metadata = extract_resource_metadata(content)
        .with_context(|| format!("Failed to parse resource metadata from {}", file.display()))?;
    let kind = metadata.kind.as_deref().context("Missing 'kind' field in resource")?;
    let kind = kind.to_lowercase();
    let name = metadata
        .name
        .as_deref()
        .context("Missing 'metadata.name' field in resource")?;
    let namespace = metadata.namespace.as_deref();
    let document: serde_yaml::Value =
        serde_yaml::from_str(content).with_context(|| format!("Failed to parse YAML from {}", file.display()))?;

    let exists = match client.get(&kind, namespace, name).await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    };

    let response = if exists {
        if crate::admin_apply::has_explicit_resource_version(&document) {
            client.update(&kind, namespace, name, content.to_string()).await?
        } else {
            crate::admin_apply::put_unconditional_with_conflict_retry(|| {
                client.update(&kind, namespace, name, content.to_string())
            })
            .await?
        }
    } else {
        client.create(&kind, namespace, content.to_string()).await?
    };

    handle_response(response).await.map(|_| ()).with_context(|| {
        format!(
            "Admin API failed to apply {}/{} in namespace {}",
            kind,
            name,
            namespace.unwrap_or("<cluster-scoped>")
        )
    })
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

#[cfg(test)]
mod tests {
    use super::split_yaml_docs;

    #[test]
    fn split_yaml_docs_preserves_each_nonempty_document() {
        let documents = split_yaml_docs("---\nkind: A\n---\n---\nkind: B\n");

        assert_eq!(documents.len(), 2);
        assert!(documents[0].contains("kind: A"));
        assert!(documents[1].contains("kind: B"));
    }

    #[test]
    fn split_yaml_docs_does_not_split_inline_dashes() {
        let documents = split_yaml_docs("kind: A\nvalue: --not-a-separator\n");

        assert_eq!(documents.len(), 1);
        assert!(documents[0].contains("--not-a-separator"));
    }
}
