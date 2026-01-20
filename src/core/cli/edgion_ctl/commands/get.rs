use anyhow::Result;
use serde_json::Value;

use crate::core::cli::edgion_ctl::client::{parse_json_response, EdgionClient};
use crate::core::cli::edgion_ctl::output::{print_error, print_resource, print_resource_list, OutputFormat};
use crate::core::cli::edgion_ctl::TargetType;

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
    let component = match client.target() {
        TargetType::Center | TargetType::Server => "controller",
        TargetType::Client => "gateway",
    };
    eprintln!();
    eprintln!("Hint: edgion-ctl is trying to connect to: {}", client.base_url());
    eprintln!("      Target: {} ({})", client.target(), component);
    eprintln!("      Use --server to specify a different address, e.g.:");
    eprintln!(
        "        ./edgion-ctl -t {} --server {} get httproute",
        client.target(),
        client.base_url()
    );
}

/// Filter resources by namespace (client-side filtering for server/client targets)
fn filter_by_namespace(data: &mut Value, namespace: &str) {
    // First, filter the items and get the new count
    let new_count = if let Some(items) = data.get_mut("data").and_then(|d| d.as_array_mut()) {
        items.retain(|item| {
            item.get("metadata")
                .and_then(|m| m.get("namespace"))
                .and_then(|n| n.as_str())
                == Some(namespace)
        });
        Some(items.len())
    } else {
        None
    };

    // Then update count field if exists
    if let Some(count) = new_count {
        if let Some(count_field) = data.get_mut("count") {
            *count_field = Value::Number(count.into());
        }
    }
}

/// Extract resource data from response based on target type
/// - center: get returns raw JSON `{...}`
/// - server/client: get returns wrapped format `{"success": true, "data": {...}}`
fn extract_resource_data(data: Value, target: TargetType) -> Value {
    match target {
        TargetType::Center => {
            // Center returns raw resource for get single resource
            data
        }
        TargetType::Server | TargetType::Client => {
            // Server/Client returns wrapped format, extract from "data" field
            if let Some(inner_data) = data.get("data") {
                inner_data.clone()
            } else {
                data
            }
        }
    }
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
            // Try to extract error message from JSON response
            if let Ok(json) = serde_json::from_str::<Value>(&body) {
                if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
                    print_error(&format!("Failed to get resource: {}", error));
                    anyhow::bail!("Request failed");
                }
            }
            print_error(&format!("Failed to get resource: {}", body));
            anyhow::bail!("Request failed");
        }
    }

    let data = parse_json_response(resp).await?;
    // Handle different response formats based on target
    let resource_data = extract_resource_data(data, client.target());
    print_resource(&resource_data, output)?;

    Ok(())
}

/// List resources in a namespace
async fn list_namespaced(client: &EdgionClient, kind: &str, namespace: &str, output: OutputFormat) -> Result<()> {
    match client.target() {
        TargetType::Center => {
            // Center target: use server-side filtering
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
        }
        TargetType::Server | TargetType::Client => {
            // Server/Client targets: fetch all and filter on client side
            let resp = match client.list_all(kind).await {
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

            let mut data = parse_json_response(resp).await?;
            // Apply client-side namespace filtering
            filter_by_namespace(&mut data, namespace);
            print_resource_list(&data, output)?;
        }
    }

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
