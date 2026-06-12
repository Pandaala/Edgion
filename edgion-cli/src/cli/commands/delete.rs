use anyhow::{Context, Result};
use std::path::Path;
use tokio::fs;

use crate::cli::client::{handle_response, EdgionClient};
use crate::cli::output::{print_error, print_success};
use edgion_common::utils::extract_resource_metadata;

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

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn cli_parses_delete_with_kind_and_name_positional() {
        let cli = Cli::try_parse_from(["edgion-cli", "delete", "httproute", "my-route"])
            .expect("delete with positional kind+name should parse");
        match cli.command {
            Commands::Delete {
                kind,
                name,
                namespace,
                file,
            } => {
                assert_eq!(kind.as_deref(), Some("httproute"));
                assert_eq!(name.as_deref(), Some("my-route"));
                assert!(namespace.is_none());
                assert!(file.is_none());
            }
            other => panic!("expected Delete, got {:?}", other),
        }
    }

    #[test]
    fn cli_parses_delete_with_namespace_short_flag() {
        let cli = Cli::try_parse_from(["edgion-cli", "delete", "service", "svc-1", "-n", "edgion-system"])
            .expect("delete with -n should parse");
        match cli.command {
            Commands::Delete { namespace, .. } => {
                assert_eq!(namespace.as_deref(), Some("edgion-system"));
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn cli_parses_delete_with_file_flag_only() {
        let cli = Cli::try_parse_from(["edgion-cli", "delete", "--file", "route.yaml"])
            .expect("delete with --file only should parse");
        match cli.command {
            Commands::Delete { kind, name, file, .. } => {
                assert!(kind.is_none());
                assert!(name.is_none());
                assert_eq!(file.as_deref(), Some("route.yaml"));
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn cli_parses_delete_with_no_args_succeeds_at_parse_time() {
        // All positional args and --file are optional at the clap layer; the
        // mutual-exclusion check happens inside the runtime delete() function.
        // This documents that runtime invariant rather than enforcing it via clap.
        let cli = Cli::try_parse_from(["edgion-cli", "delete"]).expect("parse-only check succeeds");
        match cli.command {
            Commands::Delete {
                kind,
                name,
                namespace,
                file,
            } => {
                assert!(kind.is_none() && name.is_none() && namespace.is_none() && file.is_none());
            }
            _ => panic!("expected Delete"),
        }
    }
}
