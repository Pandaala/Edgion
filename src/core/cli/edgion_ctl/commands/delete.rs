use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

use crate::core::cli::edgion_ctl::client::{handle_response, EdgionClient};
use crate::core::cli::edgion_ctl::output::{print_error, print_success};
use crate::core::utils::extract_resource_metadata;

/// Delete command - deletes resources
pub async fn delete(
    client: &EdgionClient,
    kind: Option<&str>,
    name: Option<&str>,
    namespace: Option<&str>,
    file: Option<&str>,
) -> Result<()> {
    if let Some(file_path) = file {
        // Delete resource specified in file
        delete_from_file(client, file_path).await
    } else if let (Some(k), Some(n)) = (kind, name) {
        // Delete specific resource
        delete_resource(client, k, namespace, n).await
    } else {
        anyhow::bail!("Must specify either --file or both kind and name");
    }
}

/// Delete a specific resource
async fn delete_resource(client: &EdgionClient, kind: &str, namespace: Option<&str>, name: &str) -> Result<()> {
    let resp = client
        .delete(kind, namespace, name)
        .await
        .with_context(|| format!("Failed to delete {} {}", kind, name))?;

    match handle_response(resp).await {
        Ok(_) => {
            let ns_str = namespace.map(|n| format!(" in namespace {}", n)).unwrap_or_default();
            print_success(&format!("{} {}{} deleted", kind, name, ns_str));
        }
        Err(e) => {
            print_error(&format!("Failed to delete {} {}: {}", kind, name, e));
            anyhow::bail!(e);
        }
    }

    Ok(())
}

/// Delete resource specified in YAML file
async fn delete_from_file(client: &EdgionClient, file_path: &str) -> Result<()> {
    let path = Path::new(file_path);

    if !path.exists() {
        anyhow::bail!("File not found: {}", file_path);
    }

    // Read file content
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file: {}", file_path))?;

    // Extract metadata
    let metadata =
        extract_resource_metadata(&content).with_context(|| format!("Failed to parse metadata from {}", file_path))?;

    let kind = metadata.kind.as_ref().context("Missing 'kind' field in resource")?;
    let name = metadata
        .name
        .as_ref()
        .context("Missing 'metadata.name' field in resource")?;
    let namespace = metadata.namespace.as_deref();

    // Delete the resource
    delete_resource(client, kind, namespace, name).await
}
