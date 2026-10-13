use anyhow::Result;
use serde_json::Value;

use crate::cli::client::{diagnostic_origin, parse_json_response, EdgionClient};
use crate::cli::output::{print_error, print_resource, print_resource_list, OutputFormat};
use crate::cli::TargetType;

/// Optional knobs for `get`, threaded from clap to the per-list helpers.
#[derive(Default)]
pub struct GetOptions {
    pub metadata_only: bool,
    pub limit: Option<u32>,
    pub continue_token: Option<String>,
    pub all: bool,
}

/// Get command - retrieves resources
pub async fn get(
    client: &EdgionClient,
    kind: &str,
    name: Option<&str>,
    namespace: Option<&str>,
    output: OutputFormat,
    opts: GetOptions,
) -> Result<()> {
    // Target compatibility: --metadata-only is only meaningful with --target center.
    // The check lives here (in the command layer), NOT inside EdgionClient — the
    // HTTP client is a generic transport and should not carry CLI UX rules.
    if opts.metadata_only && client.target() != TargetType::Center {
        eprintln!(
            "Error: --metadata-only only works with --target center (got {:?}).",
            client.target()
        );
        std::process::exit(2);
    }

    if let Some(resource_name) = name {
        // Get specific resource — opts are ignored for single-resource gets
        get_resource(client, kind, namespace, resource_name, output).await
    } else if let Some(ns) = namespace {
        // List resources in namespace
        list_namespaced(client, kind, ns, output, &opts).await
    } else {
        // List all resources across namespaces
        list_all(client, kind, output, &opts).await
    }
}

/// Print connection hint when request fails
fn print_connection_hint(client: &EdgionClient) {
    let origin = diagnostic_origin(client.base_url());
    let component = match client.target() {
        TargetType::Center | TargetType::Server => "controller",
        TargetType::Client => "gateway",
    };
    eprintln!();
    eprintln!("Hint: edgion-cli is trying to connect to: {}", origin);
    eprintln!("      Target: {} ({})", client.target(), component);
    eprintln!("      Use --server to specify a different address, e.g.:");
    eprintln!(
        "        ./edgion-cli -t {} --server {} get httproute",
        client.target(),
        origin
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

/// Apply command-owned namespace selection without changing server pagination.
fn filter_list_page(data: &mut Value, target: TargetType, namespace: Option<&str>) {
    if matches!(target, TargetType::Server | TargetType::Client) {
        if let Some(namespace) = namespace {
            filter_by_namespace(data, namespace);
        }
    }
}

/// Extract resource data from response based on target type
/// - center: get returns raw JSON `{...}`
/// - server/client: get returns wrapped format `{"success": true, "data": {...}}`
fn extract_resource_data(data: Value, target: TargetType) -> Value {
    match target {
        TargetType::Center => data,
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

/// Map HTTP error status to a user-friendly CLI error message and return an
/// `anyhow::Error`. Caller uses `?` to bail.
///
/// Cases:
///   - HTTP 410: stale K8s continue token (mapped by controller's StalePagination)
///   - HTTP 400 with "continue" or "stale" in body: same surfaced by non-K8s
///     backends or by validation layer
///   - HTTP 5xx: generic "backend unavailable / unsupported" hint
///   - Other: print body verbatim
async fn handle_list_error_status(resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let body_lower = body.to_lowercase();

    if status.as_u16() == 410 {
        print_error("Pagination token is invalid or expired. Re-run without --continue to start over.");
        return anyhow::anyhow!("Stale pagination token (HTTP 410)");
    }
    if status.as_u16() == 400
        && (body_lower.contains("continue") || body_lower.contains("stale") || body_lower.contains("expired"))
    {
        print_error("Pagination token is invalid or expired. Re-run without --continue to start over.");
        return anyhow::anyhow!("Stale pagination token (HTTP 400)");
    }
    if status.is_server_error() {
        print_error(&format!(
            "Controller returned {} — it might be unavailable or the backend doesn't support this operation. \
             Run 'edgion-cli server-info' to check controller status and mode.",
            status
        ));
        return anyhow::anyhow!("Server error: {}", status);
    }
    print_error(&format!("Failed to list resources: {} ({})", status, body));
    anyhow::anyhow!("Request failed: {}", status)
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
    let resource_data = extract_resource_data(data, client.target());
    print_resource(&resource_data, output)?;

    Ok(())
}

/// Auto-paginate: loop until `continue_token` is null, accumulate (for
/// yaml/json) or stream per-page (for table/wide).
async fn list_all_paginated(
    client: &EdgionClient,
    kind: &str,
    namespace: Option<&str>,
    output: OutputFormat,
    opts: &GetOptions,
) -> Result<()> {
    let mut accumulated_items: Vec<Value> = Vec::new();
    let mut continue_token: Option<String> = opts.continue_token.clone();
    let mut page_index = 0usize;
    let mut printed_rows = false;

    loop {
        let resp = match (namespace, opts.metadata_only && client.target() == TargetType::Center) {
            (Some(ns), true) => {
                client
                    .list_namespaced_keys(kind, ns, opts.limit, continue_token.as_deref())
                    .await
            }
            (Some(ns), false) => {
                client
                    .list_namespaced(kind, ns, opts.limit, continue_token.as_deref())
                    .await
            }
            (None, true) => client.list_all_keys(kind, opts.limit, continue_token.as_deref()).await,
            (None, false) => client.list_all(kind, opts.limit, continue_token.as_deref()).await,
        };
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                print_error(&format!("Failed to list {} (page {}): {}", kind, page_index + 1, e));
                print_connection_hint(client);
                anyhow::bail!("Request failed");
            }
        };
        if !resp.status().is_success() {
            // Delegate to the shared helper so --all paths get the same
            // friendly 5xx hint / stale-token message as single-page paths.
            return Err(handle_list_error_status(resp).await);
        }
        let mut data = parse_json_response(resp).await?;
        filter_list_page(&mut data, client.target(), namespace);

        match output {
            OutputFormat::Table | OutputFormat::Wide => {
                let count = crate::cli::output::print_resource_list_page_streaming(&data, output, printed_rows)?;
                printed_rows |= count > 0;
            }
            OutputFormat::Json | OutputFormat::Yaml => {
                if let Some(items) = data.get("data").and_then(|d| d.as_array()) {
                    accumulated_items.extend(items.iter().cloned());
                }
            }
        }

        continue_token = data
            .get("continue_token")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        page_index += 1;
        if continue_token.is_none() {
            break;
        }
    }

    if matches!(output, OutputFormat::Json | OutputFormat::Yaml) {
        crate::cli::output::print_resource_list_all(accumulated_items, output)?;
    }
    Ok(())
}

/// List resources in a namespace
async fn list_namespaced(
    client: &EdgionClient,
    kind: &str,
    namespace: &str,
    output: OutputFormat,
    opts: &GetOptions,
) -> Result<()> {
    if opts.all {
        return list_all_paginated(client, kind, Some(namespace), output, opts).await;
    }
    match client.target() {
        TargetType::Center => {
            // Center target: use server-side filtering; branch on metadata_only.
            let resp = if opts.metadata_only {
                client
                    .list_namespaced_keys(kind, namespace, opts.limit, opts.continue_token.as_deref())
                    .await
            } else {
                client
                    .list_namespaced(kind, namespace, opts.limit, opts.continue_token.as_deref())
                    .await
            };
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    print_error(&format!("Failed to list {} in namespace {}: {}", kind, namespace, e));
                    print_connection_hint(client);
                    anyhow::bail!("Request failed");
                }
            };

            if !resp.status().is_success() {
                return Err(handle_list_error_status(resp).await);
            }

            let data = parse_json_response(resp).await?;
            print_resource_list(&data, output)?;
        }
        TargetType::Server | TargetType::Client => {
            // Server/Client targets: fetch all and filter on client side
            let resp = match client.list_all(kind, opts.limit, opts.continue_token.as_deref()).await {
                Ok(r) => r,
                Err(e) => {
                    print_error(&format!("Failed to list {} in namespace {}: {}", kind, namespace, e));
                    print_connection_hint(client);
                    anyhow::bail!("Request failed");
                }
            };

            if !resp.status().is_success() {
                return Err(handle_list_error_status(resp).await);
            }

            let mut data = parse_json_response(resp).await?;
            // Apply client-side namespace filtering
            filter_list_page(&mut data, client.target(), Some(namespace));
            print_resource_list(&data, output)?;
        }
    }

    Ok(())
}

/// List all resources across namespaces
async fn list_all(client: &EdgionClient, kind: &str, output: OutputFormat, opts: &GetOptions) -> Result<()> {
    if opts.all {
        return list_all_paginated(client, kind, None, output, opts).await;
    }
    let resp = if opts.metadata_only && client.target() == TargetType::Center {
        client
            .list_all_keys(kind, opts.limit, opts.continue_token.as_deref())
            .await
    } else {
        client.list_all(kind, opts.limit, opts.continue_token.as_deref()).await
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            print_error(&format!("Failed to list {}: {}", kind, e));
            print_connection_hint(client);
            anyhow::bail!("Request failed");
        }
    };

    if !resp.status().is_success() {
        return Err(handle_list_error_status(resp).await);
    }

    let data = parse_json_response(resp).await?;
    print_resource_list(&data, output)?;

    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/cli/commands/get_test.rs"]
mod tests;
