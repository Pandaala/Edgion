//! Resource Diff Tool
//!
//! Validates resource synchronization between edgion-controller and edgion-gateway
//! by comparing resources via their admin APIs and checking Secret references.

use clap::Parser;
use colored::Colorize;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process;
use std::time::Duration;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(name = "resource_diff")]
#[command(about = "Verify resource synchronization between controller and gateway")]
struct Args {
    /// Controller admin API URL
    #[arg(long, default_value = "http://127.0.0.1:5800")]
    controller_url: String,

    /// Gateway admin API URL
    #[arg(long, default_value = "http://127.0.0.1:5810")]
    gateway_url: String,

    /// Output format (text or json)
    #[arg(long, default_value = "text")]
    output_format: String,

    /// Request timeout in seconds
    #[arg(long, default_value = "5")]
    timeout: u64,

    /// Maximum retry attempts for failed requests
    #[arg(long, default_value = "3")]
    max_retries: u32,

    /// Resource kinds to skip (not synced to Gateway by design)
    /// Default: ReferenceGrant,Secret
    #[arg(long, value_delimiter = ',', default_value = "ReferenceGrant,Secret")]
    skip_kinds: Vec<String>,
}

/// Resource identifier (kind, name, namespace)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResourceId {
    kind: String,
    name: String,
    namespace: Option<String>,
}

impl ResourceId {
    fn new(kind: &str, name: String, namespace: Option<String>) -> Self {
        Self {
            kind: kind.to_string(),
            name,
            namespace,
        }
    }

    fn display(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}/{}", ns, self.name),
            None => self.name.clone(),
        }
    }
}

/// Resource difference for a specific kind
#[derive(Debug, Default)]
struct ResourceDiff {
    kind: String,
    missing_in_gateway: Vec<String>,
    extra_in_gateway: Vec<String>,
}

/// Secret reference issue
#[derive(Debug)]
struct SecretIssue {
    resource: String,
    kind: String,
    missing_secrets: Vec<String>,
}

/// Overall diff result
#[derive(Debug, Default)]
struct DiffResult {
    differences: Vec<ResourceDiff>,
    secret_issues: Vec<SecretIssue>,
}

impl DiffResult {
    fn has_issues(&self) -> bool {
        !self.differences.is_empty() || !self.secret_issues.is_empty()
    }

    fn total_issues(&self) -> usize {
        self.differences
            .iter()
            .map(|d| d.missing_in_gateway.len() + d.extra_in_gateway.len())
            .sum::<usize>()
            + self.secret_issues.len()
    }
}

/// Controller and Gateway API list response (both have the same format)
#[derive(Deserialize, Debug)]
struct ListApiResponse {
    success: bool,
    data: Option<Vec<serde_json::Value>>,
    #[allow(dead_code)]
    count: usize,
    error: Option<String>,
}

/// Generic resource with metadata
#[derive(Deserialize, Debug)]
struct Resource {
    metadata: Metadata,
    #[serde(flatten)]
    rest: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct Metadata {
    name: String,
    namespace: Option<String>,
}

/// Secret reference (used for validation)
#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct SecretReference {
    name: String,
    namespace: Option<String>,
}

/// Admin API client
struct AdminClient {
    client: Client,
    controller_url: String,
    gateway_url: String,
    max_retries: u32,
}

impl AdminClient {
    fn new(controller_url: String, gateway_url: String, timeout: Duration, max_retries: u32) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            controller_url,
            gateway_url,
            max_retries,
        }
    }

    /// Fetch resources from controller with retry
    async fn fetch_from_controller(&self, endpoint: &str) -> Result<Vec<Resource>, String> {
        let url = format!("{}{}", self.controller_url, endpoint);

        for attempt in 1..=self.max_retries {
            match self.client.get(&url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ListApiResponse>().await {
                            Ok(api_resp) => {
                                if api_resp.success {
                                    let json_values = api_resp.data.unwrap_or_default();
                                    let resources: Vec<Resource> = json_values
                                        .into_iter()
                                        .filter_map(|v| serde_json::from_value(v).ok())
                                        .collect();
                                    return Ok(resources);
                                } else {
                                    return Err(format!("API error: {}", api_resp.error.unwrap_or_default()));
                                }
                            }
                            Err(e) => return Err(format!("JSON parse error: {}", e)),
                        }
                    } else if attempt == self.max_retries {
                        return Err(format!("HTTP error: {}", response.status()));
                    }
                }
                Err(e) => {
                    if attempt == self.max_retries {
                        return Err(format!("Request failed: {}", e));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
        }

        Err("Max retries exceeded".to_string())
    }

    /// Fetch resources from gateway with retry
    async fn fetch_from_gateway(&self, endpoint: &str) -> Result<Vec<Resource>, String> {
        let url = format!("{}{}", self.gateway_url, endpoint);

        for attempt in 1..=self.max_retries {
            match self.client.get(&url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ListApiResponse>().await {
                            Ok(gateway_resp) => {
                                if gateway_resp.success {
                                    let json_values = gateway_resp.data.unwrap_or_default();
                                    let resources: Vec<Resource> = json_values
                                        .into_iter()
                                        .filter_map(|v| serde_json::from_value(v).ok())
                                        .collect();
                                    return Ok(resources);
                                } else {
                                    return Err(format!("API error: {}", gateway_resp.error.unwrap_or_default()));
                                }
                            }
                            Err(e) => return Err(format!("JSON parse error: {}", e)),
                        }
                    } else if attempt == self.max_retries {
                        return Err(format!("HTTP error: {}", response.status()));
                    }
                }
                Err(e) => {
                    if attempt == self.max_retries {
                        return Err(format!("Request failed: {}", e));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
        }

        Err("Max retries exceeded".to_string())
    }

    /// Check if controller is reachable
    async fn check_controller_health(&self) -> bool {
        let url = format!("{}/health", self.controller_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Check if gateway is reachable
    async fn check_gateway_health(&self) -> bool {
        let url = format!("{}/health", self.gateway_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

/// Resource types to compare (namespaced resources that exist in both Controller and Gateway)
const RESOURCE_TYPES: &[(&str, &str, &str)] = &[
    // (Kind, Controller endpoint, Gateway endpoint)
    (
        "HTTPRoute",
        "/api/v1/namespaced/HTTPRoute",
        "/configclient/httproute/list",
    ),
    (
        "GRPCRoute",
        "/api/v1/namespaced/GRPCRoute",
        "/configclient/grpcroute/list",
    ),
    ("TCPRoute", "/api/v1/namespaced/TCPRoute", "/configclient/tcproute/list"),
    ("UDPRoute", "/api/v1/namespaced/UDPRoute", "/configclient/udproute/list"),
    ("TLSRoute", "/api/v1/namespaced/TLSRoute", "/configclient/tlsroute/list"),
    ("Service", "/api/v1/namespaced/Service", "/configclient/service/list"),
    (
        "EndpointSlice",
        "/api/v1/namespaced/EndpointSlice",
        "/configclient/endpointslice/list",
    ),
    (
        "EdgionTls",
        "/api/v1/namespaced/EdgionTls",
        "/configclient/edgiontls/list",
    ),
    (
        "EdgionPlugins",
        "/api/v1/namespaced/EdgionPlugins",
        "/configclient/edgionplugins/list",
    ),
    (
        "EdgionStreamPlugins",
        "/api/v1/namespaced/EdgionStreamPlugins",
        "/configclient/edgionstreamplugins/list",
    ),
    (
        "ReferenceGrant",
        "/api/v1/namespaced/ReferenceGrant",
        "/configclient/referencegrants/list",
    ),
    (
        "BackendTLSPolicy",
        "/api/v1/namespaced/BackendTLSPolicy",
        "/configclient/backendtlspolicies/list",
    ),
    ("LinkSys", "/api/v1/namespaced/LinkSys", "/configclient/linksys/list"),
    (
        "PluginMetaData",
        "/api/v1/namespaced/PluginMetaData",
        "/configclient/pluginmetadata/list",
    ),
];

/// Compute the set difference between controller-side and gateway-side resources
/// of a single kind. Returns a `ResourceDiff` (which may be empty). Pure: no I/O.
fn compute_resource_diff(kind: &str, controller: &[Resource], gateway: &[Resource]) -> ResourceDiff {
    let controller_ids: HashSet<ResourceId> = controller
        .iter()
        .map(|r| ResourceId::new(kind, r.metadata.name.clone(), r.metadata.namespace.clone()))
        .collect();
    let gateway_ids: HashSet<ResourceId> = gateway
        .iter()
        .map(|r| ResourceId::new(kind, r.metadata.name.clone(), r.metadata.namespace.clone()))
        .collect();
    let missing_in_gateway: Vec<String> = controller_ids.difference(&gateway_ids).map(|id| id.display()).collect();
    let extra_in_gateway: Vec<String> = gateway_ids.difference(&controller_ids).map(|id| id.display()).collect();
    ResourceDiff {
        kind: kind.to_string(),
        missing_in_gateway,
        extra_in_gateway,
    }
}

/// Note: Gateway and GatewayClass resources are Kubernetes-level resources managed by Controller only.
/// They are not synced to Gateway's config client, so we skip comparing them.
///
/// Compare resources between controller and gateway
async fn compare_resources(client: &AdminClient, skip_kinds: &[String]) -> DiffResult {
    let mut result = DiffResult::default();

    for (kind, controller_endpoint, gateway_endpoint) in RESOURCE_TYPES {
        // Skip kinds that are not synced to Gateway by design
        if skip_kinds.iter().any(|s| s.eq_ignore_ascii_case(kind)) {
            continue;
        }

        let controller_resources = match client.fetch_from_controller(controller_endpoint).await {
            Ok(res) => res,
            Err(e) => {
                eprintln!("{} Failed to fetch {} from controller: {}", "⚠".yellow(), kind, e);
                continue;
            }
        };

        let gateway_resources = match client.fetch_from_gateway(gateway_endpoint).await {
            Ok(res) => res,
            Err(e) => {
                eprintln!("{} Failed to fetch {} from gateway: {}", "⚠".yellow(), kind, e);
                continue;
            }
        };

        let diff = compute_resource_diff(kind, &controller_resources, &gateway_resources);
        if !diff.missing_in_gateway.is_empty() || !diff.extra_in_gateway.is_empty() {
            result.differences.push(diff);
        }
    }

    result
}

/// For a list of `certificateRefs` JSON values, return the `ns/name` of each ref
/// whose Secret is not present in `secret_ids`. A ref with no explicit `namespace`
/// falls back to `default_ns`. Refs without a `name` are skipped. Pure: no I/O.
fn collect_missing_cert_refs(
    cert_refs: &[serde_json::Value],
    default_ns: &str,
    secret_ids: &HashSet<(String, String)>,
) -> Vec<String> {
    let mut missing = Vec::new();
    for cert_ref in cert_refs {
        if let Some(name) = cert_ref.get("name").and_then(|v| v.as_str()) {
            let ns = cert_ref.get("namespace").and_then(|v| v.as_str()).unwrap_or(default_ns);
            if !secret_ids.contains(&(ns.to_string(), name.to_string())) {
                missing.push(format!("{}/{}", ns, name));
            }
        }
    }
    missing
}

/// Validate Secret references in resources
async fn validate_secret_references(client: &AdminClient) -> Vec<SecretIssue> {
    let mut issues = Vec::new();

    // Fetch all Secrets from controller
    let secrets = match client.fetch_from_controller("/api/v1/namespaced/Secret").await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{} Failed to fetch Secrets from controller: {}", "⚠".yellow(), e);
            return issues;
        }
    };

    let secret_ids: HashSet<(String, String)> = secrets
        .iter()
        .map(|s| {
            let ns = s
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| "edgion-default".to_string());
            (ns, s.metadata.name.clone())
        })
        .collect();

    // Check EdgionTls certificate_refs
    if let Ok(tls_resources) = client.fetch_from_controller("/api/v1/namespaced/EdgionTls").await {
        for tls in tls_resources {
            let mut missing = Vec::new();

            // Extract certificate_refs from spec
            if let Some(spec) = tls.rest.get("spec") {
                if let Some(cert_refs) = spec.get("certificateRefs").and_then(|v| v.as_array()) {
                    let default_ns = tls.metadata.namespace.as_deref().unwrap_or("edgion-default");
                    missing.extend(collect_missing_cert_refs(cert_refs, default_ns, &secret_ids));
                }
            }

            if !missing.is_empty() {
                issues.push(SecretIssue {
                    resource: format!(
                        "{}/{}",
                        tls.metadata.namespace.unwrap_or_else(|| "edgion-default".to_string()),
                        tls.metadata.name
                    ),
                    kind: "EdgionTls".to_string(),
                    missing_secrets: missing,
                });
            }
        }
    }

    // Check Gateway TLS certificate_refs
    if let Ok(gateways) = client.fetch_from_controller("/api/v1/cluster/Gateway").await {
        for gw in gateways {
            let mut missing = Vec::new();

            if let Some(spec) = gw.rest.get("spec") {
                if let Some(listeners) = spec.get("listeners").and_then(|v| v.as_array()) {
                    for listener in listeners {
                        if let Some(tls) = listener.get("tls") {
                            if let Some(cert_refs) = tls.get("certificateRefs").and_then(|v| v.as_array()) {
                                missing.extend(collect_missing_cert_refs(cert_refs, "edgion-default", &secret_ids));
                            }
                        }
                    }
                }
            }

            if !missing.is_empty() {
                issues.push(SecretIssue {
                    resource: gw.metadata.name.clone(),
                    kind: "Gateway".to_string(),
                    missing_secrets: missing,
                });
            }
        }
    }

    issues
}

/// Print text report
fn print_text_report(result: &DiffResult, controller_url: &str, gateway_url: &str) {
    println!("\n{}", "=".repeat(50));
    println!("  {}", "Resource Synchronization Report".bold());
    println!("{}\n", "=".repeat(50));

    println!("Controller: {}", controller_url);
    println!("Gateway:    {}\n", gateway_url);

    // Resource differences
    if result.differences.is_empty() {
        println!("{} All resource types are synchronized", "✓".green());
    } else {
        for diff in &result.differences {
            if diff.missing_in_gateway.is_empty() && diff.extra_in_gateway.is_empty() {
                println!("{} {}: synchronized", "✓".green(), diff.kind);
            } else {
                println!(
                    "{} {}: {} missing, {} extra",
                    "✗".red(),
                    diff.kind,
                    diff.missing_in_gateway.len(),
                    diff.extra_in_gateway.len()
                );

                if !diff.missing_in_gateway.is_empty() {
                    println!("    Missing in Gateway:");
                    for resource in &diff.missing_in_gateway {
                        println!("      - {}", resource.red());
                    }
                }

                if !diff.extra_in_gateway.is_empty() {
                    println!("    Extra in Gateway:");
                    for resource in &diff.extra_in_gateway {
                        println!("      - {}", resource.yellow());
                    }
                }
            }
        }
    }

    // Secret issues
    if !result.secret_issues.is_empty() {
        println!();
        for issue in &result.secret_issues {
            println!("{} {}: missing Secret references", "!".yellow(), issue.kind);
            println!("    Resource: {}", issue.resource);
            for secret in &issue.missing_secrets {
                println!("      - Missing Secret: {}", secret.red());
            }
        }
    }

    println!("\n{}", "=".repeat(50));
    if result.has_issues() {
        println!(
            "Summary: {} {}",
            result.total_issues().to_string().red().bold(),
            "issues found".red()
        );
    } else {
        println!(
            "{}",
            "Summary: No issues found - All resources synchronized".green().bold()
        );
    }
    println!("{}\n", "=".repeat(50));
}

/// Print JSON report
fn print_json_report(result: &DiffResult, controller_url: &str, gateway_url: &str) {
    #[derive(Serialize)]
    struct JsonReport {
        controller_url: String,
        gateway_url: String,
        differences: Vec<JsonDiff>,
        secret_issues: Vec<JsonSecretIssue>,
        total_issues: usize,
        synchronized: bool,
    }

    #[derive(Serialize)]
    struct JsonDiff {
        kind: String,
        missing_in_gateway: Vec<String>,
        extra_in_gateway: Vec<String>,
    }

    #[derive(Serialize)]
    struct JsonSecretIssue {
        resource: String,
        kind: String,
        missing_secrets: Vec<String>,
    }

    let report = JsonReport {
        controller_url: controller_url.to_string(),
        gateway_url: gateway_url.to_string(),
        differences: result
            .differences
            .iter()
            .map(|d| JsonDiff {
                kind: d.kind.clone(),
                missing_in_gateway: d.missing_in_gateway.clone(),
                extra_in_gateway: d.extra_in_gateway.clone(),
            })
            .collect(),
        secret_issues: result
            .secret_issues
            .iter()
            .map(|i| JsonSecretIssue {
                resource: i.resource.clone(),
                kind: i.kind.clone(),
                missing_secrets: i.missing_secrets.clone(),
            })
            .collect(),
        total_issues: result.total_issues(),
        synchronized: !result.has_issues(),
    };

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Create admin client
    let client = AdminClient::new(
        args.controller_url.clone(),
        args.gateway_url.clone(),
        Duration::from_secs(args.timeout),
        args.max_retries,
    );

    // Check connectivity
    println!("Checking connectivity...");

    let (controller_ok, gateway_ok) = tokio::join!(client.check_controller_health(), client.check_gateway_health());

    if !controller_ok {
        eprintln!("{} Cannot connect to controller at {}", "✗".red(), args.controller_url);
        process::exit(2);
    }

    if !gateway_ok {
        eprintln!("{} Cannot connect to gateway at {}", "✗".red(), args.gateway_url);
        process::exit(2);
    }

    println!("{} Connected to controller and gateway\n", "✓".green());

    // Log skipped kinds
    if !args.skip_kinds.is_empty() {
        println!(
            "{} Skipping resource kinds (not synced by design): {}",
            "ℹ".blue(),
            args.skip_kinds.join(", ")
        );
    }

    // Compare resources
    println!("Comparing resources...");
    let mut result = compare_resources(&client, &args.skip_kinds).await;

    // Validate Secret references
    println!("Validating Secret references...");
    result.secret_issues = validate_secret_references(&client).await;

    // Print report
    match args.output_format.as_str() {
        "json" => print_json_report(&result, &args.controller_url, &args.gateway_url),
        _ => print_text_report(&result, &args.controller_url, &args.gateway_url),
    }

    // Exit with appropriate code
    if result.has_issues() {
        process::exit(1);
    } else {
        process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Resource` from a name and optional namespace (used by several tests below).
    fn res(name: &str, namespace: Option<&str>) -> Resource {
        let meta = match namespace {
            Some(ns) => serde_json::json!({"name": name, "namespace": ns}),
            None => serde_json::json!({"name": name}),
        };
        serde_json::from_value(serde_json::json!({"metadata": meta})).unwrap()
    }

    // -------------------------------------------------------------------------
    // ResourceId tests
    // -------------------------------------------------------------------------

    #[test]
    fn resource_id_display_with_namespace() {
        let id = ResourceId::new("HTTPRoute", "my-route".to_string(), Some("prod".to_string()));
        assert_eq!(id.display(), "prod/my-route");
    }

    #[test]
    fn resource_id_display_without_namespace() {
        let id = ResourceId::new("Gateway", "gw".to_string(), None);
        assert_eq!(id.display(), "gw");
    }

    #[test]
    fn resource_id_equal_ids_hash_to_one_slot() {
        let id1 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns".to_string()));
        let id2 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns".to_string()));
        assert_eq!(id1, id2);
        let mut set = HashSet::new();
        set.insert(id1);
        set.insert(id2);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn resource_id_different_namespaces_are_not_equal() {
        let id1 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns-a".to_string()));
        let id2 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns-b".to_string()));
        assert_ne!(id1, id2);
        let mut set = HashSet::new();
        set.insert(id1);
        set.insert(id2);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn resource_id_some_ns_vs_none_not_equal() {
        let id1 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns".to_string()));
        let id2 = ResourceId::new("HTTPRoute", "r".to_string(), None);
        assert_ne!(id1, id2);
    }

    #[test]
    fn resource_id_different_kind_same_name_ns_not_equal() {
        let id1 = ResourceId::new("HTTPRoute", "r".to_string(), Some("ns".to_string()));
        let id2 = ResourceId::new("GRPCRoute", "r".to_string(), Some("ns".to_string()));
        assert_ne!(id1, id2);
    }

    // -------------------------------------------------------------------------
    // compute_resource_diff tests
    // -------------------------------------------------------------------------

    #[test]
    fn compute_resource_diff_identical_sets_empty() {
        let controller = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let gateway = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway);
        assert!(diff.missing_in_gateway.is_empty());
        assert!(diff.extra_in_gateway.is_empty());
    }

    #[test]
    fn compute_resource_diff_missing_in_gateway() {
        let controller = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let gateway = vec![res("route-a", Some("ns"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway);
        assert_eq!(diff.missing_in_gateway, vec!["ns/route-b"]);
        assert!(diff.extra_in_gateway.is_empty());
    }

    #[test]
    fn compute_resource_diff_extra_in_gateway() {
        let controller = vec![res("route-a", Some("ns"))];
        let gateway = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway);
        assert!(diff.missing_in_gateway.is_empty());
        assert_eq!(diff.extra_in_gateway, vec!["ns/route-b"]);
    }

    #[test]
    fn compute_resource_diff_same_name_different_ns_both_missing_and_extra() {
        // controller has "route" in "ns-ctrl"; gateway has "route" in "ns-gw"
        // They do NOT match (namespace differs), so the controller one is missing
        // from gateway and the gateway one is extra.
        let controller = vec![res("route", Some("ns-ctrl"))];
        let gateway = vec![res("route", Some("ns-gw"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway);
        assert_eq!(diff.missing_in_gateway, vec!["ns-ctrl/route"]);
        assert_eq!(diff.extra_in_gateway, vec!["ns-gw/route"]);
    }

    #[test]
    fn compute_resource_diff_kind_field_matches_input() {
        let diff = compute_resource_diff("TCPRoute", &[], &[]);
        assert_eq!(diff.kind, "TCPRoute");
    }

    // -------------------------------------------------------------------------
    // DiffResult tests
    // -------------------------------------------------------------------------

    #[test]
    fn diff_result_default_has_no_issues() {
        let result = DiffResult::default();
        assert!(!result.has_issues());
        assert_eq!(result.total_issues(), 0);
    }

    #[test]
    fn diff_result_with_resource_diff_counts_individual_resources() {
        let mut result = DiffResult::default();
        result.differences.push(ResourceDiff {
            kind: "HTTPRoute".to_string(),
            missing_in_gateway: vec!["ns/a".to_string(), "ns/b".to_string()],
            extra_in_gateway: vec!["ns/c".to_string()],
        });
        assert_eq!(result.total_issues(), 3);
        assert!(result.has_issues());
    }

    #[test]
    fn diff_result_secret_issue_counts_as_one_not_by_missing_count() {
        let mut result = DiffResult::default();
        result.secret_issues.push(SecretIssue {
            resource: "some-tls".to_string(),
            kind: "EdgionTls".to_string(),
            missing_secrets: vec!["ns/sec-a".to_string(), "ns/sec-b".to_string(), "ns/sec-c".to_string()],
        });
        // The SecretIssue counts as 1 (not 3) toward total_issues.
        assert_eq!(result.total_issues(), 1);
        assert!(result.has_issues());
    }

    #[test]
    fn diff_result_mixed_diff_and_secret_issue() {
        let mut result = DiffResult::default();
        result.differences.push(ResourceDiff {
            kind: "HTTPRoute".to_string(),
            missing_in_gateway: vec!["ns/a".to_string()],
            extra_in_gateway: vec![],
        });
        result.secret_issues.push(SecretIssue {
            resource: "some-tls".to_string(),
            kind: "EdgionTls".to_string(),
            missing_secrets: vec!["ns/sec".to_string()],
        });
        assert_eq!(result.total_issues(), 2);
    }

    // -------------------------------------------------------------------------
    // Resource deserialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn resource_deserializes_with_extra_top_level_keys() {
        let v = serde_json::json!({
            "metadata": {"name": "my-route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        let r: Resource = serde_json::from_value(v).unwrap();
        assert_eq!(r.metadata.name, "my-route");
        assert_eq!(r.metadata.namespace.as_deref(), Some("default"));
        // The extra "spec" key is captured by the flattened rest field.
        assert!(r.rest.get("spec").is_some());
    }

    #[test]
    fn resource_missing_metadata_fails_to_deserialize() {
        let v = serde_json::json!({"spec": {"hostnames": ["example.com"]}});
        assert!(serde_json::from_value::<Resource>(v).is_err());
    }

    // -------------------------------------------------------------------------
    // collect_missing_cert_refs tests
    // -------------------------------------------------------------------------

    #[test]
    fn collect_missing_cert_refs_present_secret_not_reported() {
        let mut secret_ids = HashSet::new();
        secret_ids.insert(("edgion-default".to_string(), "my-cert".to_string()));
        let cert_refs = vec![serde_json::json!({"name": "my-cert"})];
        let missing = collect_missing_cert_refs(&cert_refs, "edgion-default", &secret_ids);
        assert!(missing.is_empty());
    }

    #[test]
    fn collect_missing_cert_refs_absent_secret_reported() {
        let secret_ids: HashSet<(String, String)> = HashSet::new();
        let cert_refs = vec![serde_json::json!({"name": "missing-cert"})];
        let missing = collect_missing_cert_refs(&cert_refs, "edgion-default", &secret_ids);
        assert_eq!(missing, vec!["edgion-default/missing-cert"]);
    }

    #[test]
    fn collect_missing_cert_refs_explicit_namespace_overrides_default() {
        let mut secret_ids = HashSet::new();
        secret_ids.insert(("other-ns".to_string(), "my-cert".to_string()));
        let cert_refs = vec![serde_json::json!({"name": "my-cert", "namespace": "other-ns"})];
        // The ref carries an explicit namespace "other-ns", which matches the secret.
        let missing = collect_missing_cert_refs(&cert_refs, "edgion-default", &secret_ids);
        assert!(missing.is_empty());
        // Sanity: without the secret under "other-ns", it would be reported.
        let empty_ids: HashSet<(String, String)> = HashSet::new();
        let missing2 = collect_missing_cert_refs(&cert_refs, "edgion-default", &empty_ids);
        assert_eq!(missing2, vec!["other-ns/my-cert"]);
    }

    #[test]
    fn collect_missing_cert_refs_no_name_key_skipped() {
        let secret_ids: HashSet<(String, String)> = HashSet::new();
        let cert_refs = vec![serde_json::json!({"namespace": "ns"})]; // no "name"
        let missing = collect_missing_cert_refs(&cert_refs, "edgion-default", &secret_ids);
        assert!(missing.is_empty());
    }

    #[test]
    fn collect_missing_cert_refs_empty_slice_returns_empty() {
        let secret_ids: HashSet<(String, String)> = HashSet::new();
        let missing = collect_missing_cert_refs(&[], "edgion-default", &secret_ids);
        assert!(missing.is_empty());
    }
}
