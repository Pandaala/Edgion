use anyhow::Result;

use crate::core::cli::edgion_ctl::client::{parse_json_response, EdgionClient};
use crate::core::cli::edgion_ctl::output::{print_error, print_resource, print_resource_list, OutputFormat};

/// Get command - retrieves resources
pub async fn get(
    client: &EdgionClient,
    kind: &str,
    name: Option<&str>,
    namespace: Option<&str>,
    output: OutputFormat,
) -> Result<()> {
    if let Some(resource_name) = name {
        // Get specific resource
        get_resource(client, kind, namespace, resource_name, output).await
    } else if let Some(ns) = namespace {
        // List resources in namespace
        list_namespaced(client, kind, ns, output).await
    } else {
        // List all resources across namespaces
        list_all(client, kind, output).await
    }
}

/// Print connection hint when request fails
fn print_connection_hint(client: &EdgionClient) {
    eprintln!();
    eprintln!("Hint: edgion-ctl is trying to connect to: {}", client.base_url());
    eprintln!("      Use --server to specify a different address, e.g.:");
    eprintln!("        ./edgion-ctl --server {} get httproute", client.base_url());
}

/// Get a specific resource
async fn get_resource(
    client: &EdgionClient,
    kind: &str,
    namespace: Option<&str>,
    name: &str,
    output: OutputFormat,
) -> Result<()> {
    let resp = match client.get(kind, namespace, name).await {
        Ok(r) => r,
        Err(e) => {
            print_error(&format!("Failed to get {} {}: {}", kind, name, e));
            print_connection_hint(client);
            anyhow::bail!("Request failed");
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.as_u16() == 404 {
            print_error(&format!("{} {} not found", kind, name));
            anyhow::bail!("Resource not found");
        } else {
            print_error(&format!("Failed to get resource: {}", body));
            anyhow::bail!("Request failed");
        }
    }

    let data = parse_json_response(resp).await?;
    print_resource(&data, output)?;

    Ok(())
}

/// List resources in a namespace
async fn list_namespaced(client: &EdgionClient, kind: &str, namespace: &str, output: OutputFormat) -> Result<()> {
    let resp = match client.list_namespaced(kind, namespace).await {
        Ok(r) => r,
        Err(e) => {
            print_error(&format!("Failed to list {} in namespace {}: {}", kind, namespace, e));
            print_connection_hint(client);
            anyhow::bail!("Request failed");
        }
    };

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        print_error(&format!("Failed to list resources: {}", body));
        anyhow::bail!("Request failed");
    }

    let data = parse_json_response(resp).await?;
    print_resource_list(&data, output)?;

    Ok(())
}

/// List all resources across namespaces
async fn list_all(client: &EdgionClient, kind: &str, output: OutputFormat) -> Result<()> {
    let resp = match client.list_all(kind).await {
        Ok(r) => r,
        Err(e) => {
            print_error(&format!("Failed to list {}: {}", kind, e));
            print_connection_hint(client);
            anyhow::bail!("Request failed");
        }
    };

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        print_error(&format!("Failed to list resources: {}", body));
        anyhow::bail!("Request failed");
    }

    let data = parse_json_response(resp).await?;
    print_resource_list(&data, output)?;

    Ok(())
}
