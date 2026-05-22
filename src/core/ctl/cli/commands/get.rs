use anyhow::Result;
use serde_json::Value;

use crate::core::ctl::cli::client::{parse_json_response, EdgionClient};
use crate::core::ctl::cli::output::{print_error, print_resource, print_resource_list, OutputFormat};
use crate::core::ctl::cli::TargetType;

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
    let component = match client.target() {
        TargetType::Center | TargetType::Server => "controller",
        TargetType::Client => "gateway",
    };
    eprintln!();
    eprintln!("Hint: edgion-cli is trying to connect to: {}", client.base_url());
    eprintln!("      Target: {} ({})", client.target(), component);
    eprintln!("      Use --server to specify a different address, e.g.:");
    eprintln!(
        "        ./edgion-cli -t {} --server {} get httproute",
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
    // Handle different response formats based on target
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
        let data = parse_json_response(resp).await?;

        match output {
            OutputFormat::Table | OutputFormat::Wide => {
                let suppress_header = page_index > 0;
                let _ =
                    crate::core::ctl::cli::output::print_resource_list_page_streaming(&data, output, suppress_header)?;
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
        crate::core::ctl::cli::output::print_resource_list_all(accumulated_items, output)?;
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
            filter_by_namespace(&mut data, namespace);
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
mod tests {
    use super::*;
    use crate::core::ctl::cli::{Cli, Commands};
    use clap::Parser;
    use serde_json::json;

    // ---- filter_by_namespace ----

    #[test]
    fn filter_by_namespace_keeps_only_matching_items_and_updates_count() {
        let mut data = json!({
            "data": [
                {"metadata": {"name": "a", "namespace": "ns1"}},
                {"metadata": {"name": "b", "namespace": "ns2"}},
                {"metadata": {"name": "c", "namespace": "ns1"}},
            ],
            "count": 3,
        });
        filter_by_namespace(&mut data, "ns1");
        let items = data["data"].as_array().expect("data should be array");
        assert_eq!(items.len(), 2);
        assert_eq!(data["count"].as_u64(), Some(2));
    }

    #[test]
    fn filter_by_namespace_with_no_matches_yields_empty_array() {
        let mut data = json!({
            "data": [
                {"metadata": {"name": "a", "namespace": "other"}},
            ],
            "count": 1,
        });
        filter_by_namespace(&mut data, "missing");
        assert!(data["data"].as_array().unwrap().is_empty());
        assert_eq!(data["count"].as_u64(), Some(0));
    }

    #[test]
    fn filter_by_namespace_is_no_op_when_data_is_not_array() {
        let mut data = json!({"data": "not-an-array", "count": 7});
        filter_by_namespace(&mut data, "ns1");
        // count must remain untouched when filter cannot run
        assert_eq!(data["count"].as_u64(), Some(7));
    }

    // ---- extract_resource_data ----

    #[test]
    fn extract_resource_data_center_returns_payload_unchanged() {
        let payload = json!({"kind": "HTTPRoute", "metadata": {"name": "x"}});
        let out = extract_resource_data(payload.clone(), TargetType::Center);
        assert_eq!(out, payload);
    }

    #[test]
    fn extract_resource_data_server_unwraps_data_field() {
        let inner = json!({"kind": "HTTPRoute", "metadata": {"name": "x"}});
        let wrapped = json!({"success": true, "data": inner.clone()});
        let out = extract_resource_data(wrapped, TargetType::Server);
        assert_eq!(out, inner);
    }

    #[test]
    fn extract_resource_data_client_returns_input_when_no_data_field() {
        let payload = json!({"foo": "bar"});
        let out = extract_resource_data(payload.clone(), TargetType::Client);
        assert_eq!(out, payload);
    }

    // ---- Cli parsing tests for the Get subcommand ----

    #[test]
    fn cli_parses_get_with_kind_only_lists_all_namespaces() {
        let cli = Cli::try_parse_from(["edgion-cli", "get", "httproute"]).expect("get <kind> should parse");
        match cli.command {
            Commands::Get {
                kind,
                name,
                namespace,
                output,
                metadata_only,
                limit,
                continue_token,
                all,
            } => {
                assert_eq!(kind, "httproute");
                assert!(name.is_none());
                assert!(namespace.is_none());
                assert_eq!(output, "table");
                assert!(!metadata_only);
                assert!(limit.is_none());
                assert!(continue_token.is_none());
                assert!(!all);
            }
            _ => panic!("expected Get"),
        }
    }

    #[test]
    fn cli_parses_get_with_name_namespace_and_output() {
        let cli = Cli::try_parse_from([
            "edgion-cli",
            "get",
            "httproute",
            "my-route",
            "-n",
            "default",
            "-o",
            "yaml",
        ])
        .expect("get with all flags should parse");
        match cli.command {
            Commands::Get {
                kind,
                name,
                namespace,
                output,
                metadata_only,
                limit,
                continue_token,
                all,
            } => {
                assert_eq!(kind, "httproute");
                assert_eq!(name.as_deref(), Some("my-route"));
                assert_eq!(namespace.as_deref(), Some("default"));
                assert_eq!(output, "yaml");
                assert!(!metadata_only);
                assert!(limit.is_none());
                assert!(continue_token.is_none());
                assert!(!all);
            }
            _ => panic!("expected Get"),
        }
    }

    #[test]
    fn cli_get_without_kind_fails() {
        let err = Cli::try_parse_from(["edgion-cli", "get"]).expect_err("get without kind should fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
