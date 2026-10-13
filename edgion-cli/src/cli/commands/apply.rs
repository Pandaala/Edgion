use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

use crate::cli::client::{handle_response, EdgionClient};
use crate::cli::output::{print_error, print_success};
use edgion_resources::resource::meta::extract_resource_metadata;

/// Apply command - creates or updates resources from YAML files
pub async fn apply(client: &EdgionClient, file_path: &str, dry_run: bool) -> Result<()> {
    apply_path(client, file_path, dry_run).await
}

/// Apply multiple input paths in order while reusing the same client.
pub async fn apply_many(client: &EdgionClient, file_paths: &[String], dry_run: bool) -> Result<()> {
    let mut failures = Vec::new();

    for file_path in file_paths {
        if let Err(error) = apply_path(client, file_path, dry_run).await {
            failures.push(format!("{}: {error:#}", file_path));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "Failed to apply {} of {} input path(s):\n{}",
            failures.len(),
            file_paths.len(),
            failures.join("\n")
        )
    }
}

async fn apply_path(client: &EdgionClient, file_path: &str, dry_run: bool) -> Result<()> {
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
    let mut attempted = 0;
    let mut failed = 0;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("yaml") | Some("yml")) {
                attempted += 1;
                match apply_file(client, &path, dry_run).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        failed += 1;
                        print_error(&format!("Failed to apply {}: {}", path.display(), e));
                    }
                }
            }
        }
    }

    if count > 0 {
        print_success(&format!("Applied {} resource(s) from {}", count, dir.display()));
    } else if attempted == 0 {
        print_error(&format!("No YAML files found in {}", dir.display()));
    }

    if failed > 0 {
        anyhow::bail!(
            "Failed to apply {} of {} YAML file(s) in {}",
            failed,
            attempted,
            dir.display()
        );
    }

    Ok(())
}

/// Split a YAML stream on top-level `---` separators. Returns each non-empty document.
fn split_yaml_docs(content: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if line.trim_start().starts_with("---") && line.trim_start().trim_start_matches('-').trim().is_empty() {
            if !current.trim().is_empty() {
                docs.push(std::mem::take(&mut current));
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        docs.push(current);
    }
    docs
}

/// Apply a YAML file (supports multi-document YAML separated by `---`).
async fn apply_file(client: &EdgionClient, file: &Path, dry_run: bool) -> Result<()> {
    let content = fs::read_to_string(file)
        .await
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let docs = split_yaml_docs(&content);
    if docs.is_empty() {
        anyhow::bail!("Empty YAML file: {}", file.display());
    }
    for (idx, doc) in docs.iter().enumerate() {
        apply_doc(client, file, doc, idx + 1, docs.len(), dry_run).await?;
    }
    Ok(())
}

/// Apply a single document (parsed from one YAML stream entry).
async fn apply_doc(
    client: &EdgionClient,
    file: &Path,
    content: &str,
    doc_idx: usize,
    doc_total: usize,
    dry_run: bool,
) -> Result<()> {
    let suffix = if doc_total > 1 {
        format!(" (doc {}/{})", doc_idx, doc_total)
    } else {
        String::new()
    };

    let metadata = extract_resource_metadata(content)
        .with_context(|| format!("Failed to parse metadata from {}{}", file.display(), suffix))?;

    let content = content.to_string();

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

#[cfg(test)]
#[path = "../../../tests/unit/cli/commands/apply_test.rs"]
mod tests;
