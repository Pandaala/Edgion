use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

use crate::core::cli::edgion_ctl::client::{handle_response, EdgionClient};
use crate::core::cli::edgion_ctl::output::{print_error, print_success};
use crate::core::utils::extract_resource_metadata;

/// Apply command - creates or updates resources from YAML files
pub async fn apply(client: &EdgionClient, file_path: &str, dry_run: bool) -> Result<()> {
    let path = Path::new(file_path);

    if !path.exists() {
        anyhow::bail!("File or directory not found: {}", file_path);
    }

    if path.is_dir() {
        // Apply all YAML files in directory
        apply_directory(client, path, dry_run).await
    } else {
        // Apply single file
        apply_file(client, path, dry_run).await
    }
}

/// Apply all YAML files in a directory
async fn apply_directory(client: &EdgionClient, dir: &Path, dry_run: bool) -> Result<()> {
    let mut entries = fs::read_dir(dir).await?;
    let mut count = 0;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("yaml") | Some("yml")) {
                match apply_file(client, &path, dry_run).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        print_error(&format!("Failed to apply {}: {}", path.display(), e));
                    }
                }
            }
        }
    }

    if count > 0 {
        print_success(&format!("Applied {} resource(s) from {}", count, dir.display()));
    } else {
        print_error(&format!("No YAML files found in {}", dir.display()));
    }

    Ok(())
}

/// Apply a single YAML file
async fn apply_file(client: &EdgionClient, file: &Path, dry_run: bool) -> Result<()> {
    // Read file content
    let content = fs::read_to_string(file)
        .await
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    // Extract metadata
    let metadata = extract_resource_metadata(&content)
        .with_context(|| format!("Failed to parse metadata from {}", file.display()))?;

    let kind = metadata.kind.as_ref().context("Missing 'kind' field in resource")?;
    // Convert kind to lowercase for API compatibility
    let kind = kind.to_lowercase();
    let kind = kind.as_str();
    let name = metadata
        .name
        .as_ref()
        .context("Missing 'metadata.name' field in resource")?;
    let namespace = metadata.namespace.as_deref();

    if dry_run {
        println!("Would apply {} {} in namespace {:?}", kind, name, namespace);
        return Ok(());
    }

    // Check if resource exists
    let exists = match client.get(kind, namespace, name).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    };

    // Create or update
    let (resp, action) = if exists {
        (client.update(kind, namespace, name, content).await?, "updated")
    } else {
        (client.create(kind, namespace, content).await?, "created")
    };

    // Handle response
    match handle_response(resp).await {
        Ok(_) => {
            let ns_str = namespace.map(|n| format!(" in namespace {}", n)).unwrap_or_default();
            print_success(&format!("{} {}{} {}", kind, name, ns_str, action));
        }
        Err(e) => {
            print_error(&format!("Failed to apply {} {}: {}", kind, name, e));
            anyhow::bail!(e);
        }
    }

    Ok(())
}
