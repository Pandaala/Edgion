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
    #[arg(long, default_value = "http://127.0.0.1:12101")]
    controller_url: String,

    /// Controller probe URL serving /health and /ready. These endpoints live on a
    /// dedicated probe port (default 12100), separate from the admin API
    /// (controller_url, 12101) which serves the resource endpoints.
    #[arg(long, default_value = "http://127.0.0.1:12100")]
    controller_probe_url: String,

    /// Gateway admin API URL
    #[arg(long, default_value = "http://127.0.0.1:12001")]
    gateway_url: String,

    /// Gateway probe URL serving /health and /ready. These endpoints live on a
    /// dedicated probe port (default 12000), separate from the admin API
    /// (gateway_url, 12001) which serves the /configclient/* resource endpoints.
    #[arg(long, default_value = "http://127.0.0.1:12000")]
    gateway_probe_url: String,

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

    /// Expected negative-test reference, encoded as kind:namespace/resource:namespace/secret.
    #[arg(long)]
    expected_missing_secret: Vec<String>,

    /// Bearer token for the gateway admin API. Defaults from the
    /// EDGION_GATEWAY_ADMIN_TOKEN env var (exported by the test harness).
    #[arg(long, env = "EDGION_GATEWAY_ADMIN_TOKEN")]
    gateway_token: Option<String>,

    /// Bearer token for the controller admin API. Defaults from the
    /// EDGION_ADMIN_TOKEN env var (exported by the test harness).
    #[arg(long, env = "EDGION_ADMIN_TOKEN")]
    controller_token: Option<String>,
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

fn exclude_expected_missing_secrets(issues: &mut Vec<SecretIssue>, expected: &[String]) {
    for issue in issues.iter_mut() {
        issue.missing_secrets.retain(|secret| {
            !expected
                .iter()
                .any(|entry| entry == &format!("{}:{}:{}", issue.kind, issue.resource, secret))
        });
    }
    issues.retain(|issue| !issue.missing_secrets.is_empty());
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

/// Admin list envelope; only source reads use the continuation token.
#[derive(Deserialize, Debug)]
struct ListApiResponse {
    success: bool,
    data: Option<Vec<serde_json::Value>>,
    count: usize,
    continue_token: Option<String>,
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
    #[serde(default)]
    labels: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    annotations: Option<std::collections::BTreeMap<String, String>>,
}

impl Resource {
    /// A Controller-published DNS EndpointSlice (`controller`-mode ExternalName):
    /// built in Controller memory and relayed by conf_sync, so it is never in the
    /// configuration source the Controller Admin API lists.
    fn is_controller_dns_slice(&self) -> bool {
        use edgion_resources::constants::labels::dns::{ENDPOINT_ORIGIN, ORIGIN_CONTROLLER};
        self.metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get(ENDPOINT_ORIGIN))
            .is_some_and(|origin| origin == ORIGIN_CONTROLLER)
    }

    /// The Service a DNS slice belongs to (`kubernetes.io/service-name`).
    fn owning_service(&self) -> Option<ResourceId> {
        use edgion_resources::constants::labels::k8s::SERVICE_NAME;
        let service = self.metadata.labels.as_ref()?.get(SERVICE_NAME)?;
        Some(ResourceId::new(
            "Service",
            service.clone(),
            self.metadata.namespace.clone(),
        ))
    }

    /// A `controller`-mode ExternalName Service: the only Service whose
    /// Controller-published DNS slices may legitimately exist.
    fn is_controller_dns_owner(&self) -> bool {
        use edgion_resources::constants::annotations::edgion::DNS_RESOLUTION_MODE;
        let external_name = self
            .rest
            .get("spec")
            .and_then(|spec| spec.get("type"))
            .and_then(|t| t.as_str())
            == Some("ExternalName");
        external_name
            && self
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(DNS_RESOLUTION_MODE))
                .is_some_and(|mode| mode == "controller")
    }
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
    controller_probe_url: String,
    gateway_url: String,
    gateway_probe_url: String,
    controller_token: Option<String>,
    gateway_token: Option<String>,
    max_retries: u32,
}

impl AdminClient {
    fn new(args: &Args) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(args.timeout))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            controller_url: args.controller_url.clone(),
            controller_probe_url: args.controller_probe_url.clone(),
            gateway_url: args.gateway_url.clone(),
            gateway_probe_url: args.gateway_probe_url.clone(),
            controller_token: args.controller_token.clone(),
            gateway_token: args.gateway_token.clone(),
            max_retries: args.max_retries,
        }
    }

    /// Traverse source pages completely; any incomplete scan fails the comparison.
    async fn fetch_from_controller(&self, endpoint: &str) -> Result<Vec<Resource>, String> {
        const MAX_PAGES: usize = 1_000;
        const MAX_ROWS: usize = 100_000;
        let mut remaining_bytes = 64 * 1024 * 1024;
        let mut cursor = None;
        let mut cursors = HashSet::new();
        let mut identities = HashSet::new();
        let mut resources = Vec::new();
        for _ in 0..MAX_PAGES {
            let page = self
                .fetch_controller_page(endpoint, cursor.as_deref(), &mut remaining_bytes)
                .await?;
            if page.count > 100 {
                return Err("Controller source page exceeds requested limit".into());
            }
            let next = page.continue_token.clone();
            let items = deserialize_list_response(page, "controller", endpoint)?;
            if resources.len() + items.len() > MAX_ROWS {
                return Err("Controller source scan exceeds row budget".into());
            }
            for item in items {
                if !identities.insert((item.metadata.namespace.clone(), item.metadata.name.clone())) {
                    return Err("Controller source scan returned a duplicate identity".into());
                }
                resources.push(item);
            }
            match next {
                None => return Ok(resources),
                Some(next) => {
                    if next.is_empty() || next.len() > 8192 || !cursors.insert(next.clone()) {
                        return Err("Controller source scan returned an invalid or repeated cursor".into());
                    }
                    cursor = Some(next);
                }
            }
        }
        Err("Controller source scan exceeds page budget".into())
    }

    async fn fetch_controller_page(
        &self,
        endpoint: &str,
        cursor: Option<&str>,
        remaining_bytes: &mut usize,
    ) -> Result<ListApiResponse, String> {
        let url = format!("{}{}", self.controller_url, endpoint);
        for attempt in 1..=self.max_retries {
            let mut rb = self.client.get(&url).query(&[("limit", "100")]);
            if let Some(cursor) = cursor {
                rb = rb.query(&[("continue", cursor)]);
            }
            if let Some(token) = &self.controller_token {
                rb = rb.bearer_auth(token);
            }
            match rb.send().await {
                Ok(mut response) => {
                    if response.status().is_success() {
                        let mut body = Vec::new();
                        while let Some(chunk) =
                            response.chunk().await.map_err(|_| "Controller page body read failed")?
                        {
                            if chunk.len() > *remaining_bytes || body.len() + chunk.len() > 16 * 1024 * 1024 {
                                return Err("Controller source scan exceeds byte budget".into());
                            }
                            *remaining_bytes -= chunk.len();
                            body.extend_from_slice(&chunk);
                        }
                        return serde_json::from_slice(&body).map_err(|e| format!("JSON parse error: {e}"));
                    } else if response.status() == reqwest::StatusCode::GONE || attempt == self.max_retries {
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
            let mut rb = self.client.get(&url);
            if let Some(t) = &self.gateway_token {
                rb = rb.bearer_auth(t);
            }
            match rb.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<ListApiResponse>().await {
                            Ok(api_resp) => return deserialize_list_response(api_resp, "gateway", endpoint),
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
        // /health is served by the probe port, not the admin port.
        let url = format!("{}/health", self.controller_probe_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Check if gateway is reachable
    async fn check_gateway_health(&self) -> bool {
        // /health is served by the probe port, not the admin port.
        let url = format!("{}/health", self.gateway_probe_url);
        let mut rb = self.client.get(&url);
        if let Some(t) = &self.gateway_token {
            rb = rb.bearer_auth(t);
        }
        rb.send().await.map(|r| r.status().is_success()).unwrap_or(false)
    }
}

fn deserialize_list_response(response: ListApiResponse, source: &str, endpoint: &str) -> Result<Vec<Resource>, String> {
    if !response.success {
        return Err(format!("API error: {}", response.error.unwrap_or_default()));
    }
    let json_values = response
        .data
        .ok_or_else(|| format!("Invalid API response from {source} {endpoint}: success response omitted data"))?;
    if response.count != json_values.len() {
        return Err(format!(
            "Invalid API response from {source} {endpoint}: count {} does not match {} data items",
            response.count,
            json_values.len()
        ));
    }
    deserialize_resources(json_values, source, endpoint)
}

fn deserialize_resources(
    json_values: Vec<serde_json::Value>,
    source: &str,
    endpoint: &str,
) -> Result<Vec<Resource>, String> {
    json_values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            serde_json::from_value(value)
                .map_err(|error| format!("Invalid resource at {source} {endpoint} item {index}: {error}"))
        })
        .collect()
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
        "EdgionBackend",
        "/api/v1/namespaced/EdgionBackend",
        "/configclient/edgionbackend/list",
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
        "EdgionConfigData",
        "/api/v1/namespaced/EdgionConfigData",
        "/configclient/edgionconfigdata/list",
    ),
];

/// Compute the set difference between controller-side and gateway-side resources
/// of a single kind. Returns a `ResourceDiff` (which may be empty). Pure: no I/O.
///
/// `dns_owners` are the Controller-side `controller`-mode ExternalName
/// Services; a Controller DNS slice of any other Service is residue.
fn compute_resource_diff(
    kind: &str,
    controller: &[Resource],
    gateway: &[Resource],
    dns_owners: &HashSet<ResourceId>,
) -> ResourceDiff {
    let controller_ids: HashSet<ResourceId> = controller
        .iter()
        .map(|r| ResourceId::new(kind, r.metadata.name.clone(), r.metadata.namespace.clone()))
        .collect();
    // Controller-published DNS slices are derived, never in the configuration
    // source; skip them only when the Controller-side list lacks them (a
    // sourced slice that happens to carry the label is still compared) and
    // their Service is a `controller`-mode ExternalName (a leftover slice of a
    // deleted or switched Service is reported as extra).
    let gateway_ids: HashSet<ResourceId> = gateway
        .iter()
        .map(|r| {
            (
                r,
                ResourceId::new(kind, r.metadata.name.clone(), r.metadata.namespace.clone()),
            )
        })
        .filter(|(r, id)| {
            !(kind == "EndpointSlice"
                && r.is_controller_dns_slice()
                && !controller_ids.contains(id)
                && r.owning_service().is_some_and(|owner| dns_owners.contains(&owner)))
        })
        .map(|(_, id)| id)
        .collect();
    let missing_in_gateway: Vec<String> = controller_ids.difference(&gateway_ids).map(|id| id.display()).collect();
    let extra_in_gateway: Vec<String> = gateway_ids.difference(&controller_ids).map(|id| id.display()).collect();
    ResourceDiff {
        kind: kind.to_string(),
        missing_in_gateway,
        extra_in_gateway,
    }
}

/// Controller-side `controller`-mode ExternalName Services: the only owners a
/// Controller-published DNS slice may have.
async fn controller_dns_owners(client: &AdminClient) -> Result<HashSet<ResourceId>, String> {
    let services = client
        .fetch_from_controller("/api/v1/namespaced/Service")
        .await
        .map_err(|error| format!("Failed to fetch Service from controller: {error}"))?;
    Ok(services
        .iter()
        .filter(|service| service.is_controller_dns_owner())
        .map(|service| {
            ResourceId::new(
                "Service",
                service.metadata.name.clone(),
                service.metadata.namespace.clone(),
            )
        })
        .collect())
}

/// Note: Gateway and GatewayClass resources are Kubernetes-level resources managed by Controller only.
/// They are not synced to Gateway's config client, so we skip comparing them.
///
/// Compare resources between controller and gateway
async fn compare_resources(client: &AdminClient, skip_kinds: &[String]) -> Result<DiffResult, String> {
    let mut result = DiffResult::default();

    for (kind, controller_endpoint, gateway_endpoint) in RESOURCE_TYPES {
        // Skip kinds that are not synced to Gateway by design
        if skip_kinds.iter().any(|s| s.eq_ignore_ascii_case(kind)) {
            continue;
        }

        let controller_resources = client
            .fetch_from_controller(controller_endpoint)
            .await
            .map_err(|error| format!("Failed to fetch {kind} from controller: {error}"))?;

        let gateway_resources = client
            .fetch_from_gateway(gateway_endpoint)
            .await
            .map_err(|error| format!("Failed to fetch {kind} from gateway: {error}"))?;

        // Fetched right before the slice comparison (and even when the
        // Service kind itself is skipped), so the owner set matches the slices.
        let dns_owners = if *kind == "EndpointSlice" {
            controller_dns_owners(client).await?
        } else {
            HashSet::new()
        };
        let diff = compute_resource_diff(kind, &controller_resources, &gateway_resources, &dns_owners);
        if !diff.missing_in_gateway.is_empty() || !diff.extra_in_gateway.is_empty() {
            result.differences.push(diff);
        }
    }

    Ok(result)
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
async fn validate_secret_references(client: &AdminClient) -> Result<Vec<SecretIssue>, String> {
    let mut issues = Vec::new();

    // Fetch all Secrets from controller
    let secrets = client
        .fetch_from_controller("/api/v1/namespaced/Secret")
        .await
        .map_err(|error| format!("Failed to fetch Secrets from controller: {error}"))?;

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
    let tls_resources = client
        .fetch_from_controller("/api/v1/namespaced/EdgionTls")
        .await
        .map_err(|error| format!("Failed to fetch EdgionTls from controller: {error}"))?;
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

    // Check Gateway TLS certificate_refs
    let gateways = client
        .fetch_from_controller("/api/v1/namespaced/Gateway")
        .await
        .map_err(|error| format!("Failed to fetch Gateways from controller: {error}"))?;
    for gw in gateways {
        let mut missing = Vec::new();
        let gateway_namespace = gw.metadata.namespace.as_deref().unwrap_or("edgion-default");

        if let Some(spec) = gw.rest.get("spec") {
            if let Some(listeners) = spec.get("listeners").and_then(|v| v.as_array()) {
                for listener in listeners {
                    if let Some(tls) = listener.get("tls") {
                        if let Some(cert_refs) = tls.get("certificateRefs").and_then(|v| v.as_array()) {
                            missing.extend(collect_missing_cert_refs(cert_refs, gateway_namespace, &secret_ids));
                        }
                    }
                }
            }
        }

        if !missing.is_empty() {
            issues.push(SecretIssue {
                resource: format!("{gateway_namespace}/{}", gw.metadata.name),
                kind: "Gateway".to_string(),
                missing_secrets: missing,
            });
        }
    }

    Ok(issues)
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
    edgion_common::startup::init_crypto();

    // Create admin client
    let client = AdminClient::new(&args);

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
    let mut result = match compare_resources(&client, &args.skip_kinds).await {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{} Resource comparison failed: {}", "✗".red(), error);
            process::exit(2);
        }
    };

    // Validate Secret references
    println!("Validating Secret references...");
    result.secret_issues = match validate_secret_references(&client).await {
        Ok(issues) => issues,
        Err(error) => {
            eprintln!("{} Secret reference validation failed: {}", "✗".red(), error);
            process::exit(2);
        }
    };

    exclude_expected_missing_secrets(&mut result.secret_issues, &args.expected_missing_secret);

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

    #[test]
    fn expected_missing_secret_exempts_only_the_exact_reference() {
        let mut result = DiffResult {
            differences: vec![ResourceDiff {
                kind: "Gateway".into(),
                missing_in_gateway: vec!["test/other".into()],
                extra_in_gateway: vec![],
            }],
            secret_issues: vec![
                SecretIssue {
                    kind: "Gateway".into(),
                    resource: "test/expected".into(),
                    missing_secrets: vec!["test/cert".into(), "test/other-cert".into()],
                },
                SecretIssue {
                    kind: "Gateway".into(),
                    resource: "test/other".into(),
                    missing_secrets: vec!["test/cert".into()],
                },
                SecretIssue {
                    kind: "EdgionTls".into(),
                    resource: "test/expected".into(),
                    missing_secrets: vec!["test/cert".into()],
                },
            ],
        };
        exclude_expected_missing_secrets(&mut result.secret_issues, &["Gateway:test/expected:test/cert".into()]);
        assert_eq!(result.secret_issues.len(), 3);
        assert_eq!(result.secret_issues[0].missing_secrets, ["test/other-cert"]);
        assert_eq!(result.secret_issues[1].missing_secrets, ["test/cert"]);
        assert_eq!(result.secret_issues[2].missing_secrets, ["test/cert"]);
        assert!(result.has_issues());
        assert_eq!(result.differences.len(), 1);
    }
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a `Resource` from a name and optional namespace (used by several tests below).
    fn res(name: &str, namespace: Option<&str>) -> Resource {
        let meta = match namespace {
            Some(ns) => serde_json::json!({"name": name, "namespace": ns}),
            None => serde_json::json!({"name": name}),
        };
        serde_json::from_value(serde_json::json!({"metadata": meta})).unwrap()
    }

    fn test_client(controller_url: String, controller_token: Option<String>) -> AdminClient {
        static CRYPTO: std::sync::Once = std::sync::Once::new();
        CRYPTO.call_once(edgion_common::startup::init_crypto);
        AdminClient {
            client: Client::builder()
                .timeout(Duration::from_secs(1))
                .build()
                .expect("test HTTP client should build"),
            controller_url,
            controller_probe_url: "http://127.0.0.1:1".to_string(),
            gateway_url: "http://127.0.0.1:1".to_string(),
            gateway_probe_url: "http://127.0.0.1:1".to_string(),
            controller_token,
            gateway_token: None,
            max_retries: 1,
        }
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
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway, &HashSet::new());
        assert!(diff.missing_in_gateway.is_empty());
        assert!(diff.extra_in_gateway.is_empty());
    }

    #[test]
    fn compute_resource_diff_missing_in_gateway() {
        let controller = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let gateway = vec![res("route-a", Some("ns"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway, &HashSet::new());
        assert_eq!(diff.missing_in_gateway, vec!["ns/route-b"]);
        assert!(diff.extra_in_gateway.is_empty());
    }

    #[test]
    fn compute_resource_diff_extra_in_gateway() {
        let controller = vec![res("route-a", Some("ns"))];
        let gateway = vec![res("route-a", Some("ns")), res("route-b", Some("ns"))];
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway, &HashSet::new());
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
        let diff = compute_resource_diff("HTTPRoute", &controller, &gateway, &HashSet::new());
        assert_eq!(diff.missing_in_gateway, vec!["ns-ctrl/route"]);
        assert_eq!(diff.extra_in_gateway, vec!["ns-gw/route"]);
    }

    #[test]
    fn controller_dns_slices_are_derived_not_extra() {
        let derived = || -> Resource {
            serde_json::from_value(serde_json::json!({"metadata": {
                "name": "svc-edgion-ctl-dns", "namespace": "ns",
                "labels": {
                    "edgion.io/endpoint-origin": "controller-dns",
                    "kubernetes.io/service-name": "svc"
                }
            }}))
            .unwrap()
        };
        let owner: Resource = serde_json::from_value(serde_json::json!({
            "metadata": {
                "name": "svc", "namespace": "ns",
                "annotations": {"edgion.io/dns-resolution-mode": "controller"}
            },
            "spec": {"type": "ExternalName", "externalName": "backend.example.com"}
        }))
        .unwrap();
        assert!(owner.is_controller_dns_owner());
        let owners = HashSet::from([ResourceId::new("Service", "svc".into(), Some("ns".into()))]);
        let sourced = res("svc-abc", Some("ns"));
        let diff = compute_resource_diff("EndpointSlice", &[], &[derived(), sourced], &owners);
        assert_eq!(diff.extra_in_gateway, vec!["ns/svc-abc"]);

        // A leftover slice of a Service that is gone (or left controller mode)
        // is residue.
        let diff = compute_resource_diff("EndpointSlice", &[], &[derived()], &HashSet::new());
        assert_eq!(diff.extra_in_gateway, vec!["ns/svc-edgion-ctl-dns"]);

        // A sourced slice carrying the label is still compared, not hidden.
        let labeled = || -> Resource {
            serde_json::from_value(serde_json::json!({"metadata": {
                "name": "operator", "namespace": "ns",
                "labels": {"edgion.io/endpoint-origin": "controller-dns"}
            }}))
            .unwrap()
        };
        let diff = compute_resource_diff("EndpointSlice", &[labeled()], &[labeled()], &HashSet::new());
        assert!(diff.missing_in_gateway.is_empty() && diff.extra_in_gateway.is_empty());
    }

    #[test]
    fn compute_resource_diff_kind_field_matches_input() {
        let diff = compute_resource_diff("TCPRoute", &[], &[], &HashSet::new());
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

    #[test]
    fn resource_list_rejects_a_malformed_item_instead_of_dropping_it() {
        let values = vec![
            serde_json::json!({"metadata": {"name": "valid"}}),
            serde_json::json!({"spec": {"hostnames": ["example.com"]}}),
        ];

        let error = deserialize_resources(values, "controller", "/api/v1/namespaced/HTTPRoute")
            .expect_err("a malformed resource must fail the complete list read");

        assert!(error.contains("controller /api/v1/namespaced/HTTPRoute item 1"));
    }

    #[test]
    fn successful_list_response_requires_data_and_matching_count() {
        let missing_data = ListApiResponse {
            success: true,
            data: None,
            count: 0,
            continue_token: None,
            error: None,
        };
        let error = deserialize_list_response(missing_data, "controller", "/resources")
            .expect_err("a success envelope without data must fail");
        assert!(error.contains("success response omitted data"));

        let wrong_count = ListApiResponse {
            success: true,
            data: Some(vec![serde_json::json!({"metadata": {"name": "one"}})]),
            count: 0,
            continue_token: None,
            error: None,
        };
        let error =
            deserialize_list_response(wrong_count, "gateway", "/resources").expect_err("a mismatched count must fail");
        assert!(error.contains("count 0 does not match 1 data items"));
    }

    #[tokio::test]
    async fn controller_scan_follows_encoded_cursors_and_empty_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/resources"))
            .and(header("authorization", "Bearer correct-token"))
            .respond_with(|request: &wiremock::Request| {
                let query: std::collections::HashMap<_, _> = request.url.query_pairs().into_owned().collect();
                assert_eq!(query.get("limit").map(String::as_str), Some("100"));
                let (data, next) = match query.get("continue").map(String::as_str) {
                    None => (
                        (0..10)
                            .map(|i| serde_json::json!({"metadata":{"name":format!("r{i}")}}))
                            .collect::<Vec<_>>(),
                        Some("opaque+&=/ cursor"),
                    ),
                    Some("opaque+&=/ cursor") => (vec![], Some("last")),
                    Some("last") => (vec![serde_json::json!({"metadata":{"name":"r10"}})], None),
                    other => panic!("unexpected cursor: {other:?}"),
                };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success":true,"count":data.len(),"data":data,"continue_token":next
                }))
            })
            .expect(3)
            .mount(&server)
            .await;
        let client = test_client(server.uri(), Some("correct-token".into()));
        let items = client.fetch_from_controller("/resources").await.unwrap();
        let gateway = (0..11).map(|i| res(&format!("r{i}"), None)).collect::<Vec<_>>();
        let diff = compute_resource_diff("HTTPRoute", &items, &gateway, &HashSet::new());
        assert!(diff.missing_in_gateway.is_empty());
        assert!(diff.extra_in_gateway.is_empty());
        assert_eq!(items.len(), 11);
    }

    #[tokio::test]
    async fn controller_scan_rejects_incomplete_or_inconsistent_later_pages() {
        for (case, expected) in [
            ("loop", "repeated cursor"),
            ("duplicate", "duplicate identity"),
            ("count", "does not match"),
            ("failure", "503"),
            ("expired", "410"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/resources"))
                .respond_with(move |request: &wiremock::Request| {
                    if !request.url.query_pairs().any(|(key, _)| key == "continue") {
                        return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                            "success":true,"count":1,"data":[{"metadata":{"name":"one"}}],"continue_token":"next"
                        }));
                    }
                    match case {
                        "failure" => ResponseTemplate::new(503),
                        "expired" => ResponseTemplate::new(410),
                        "loop" => ResponseTemplate::new(200).set_body_json(
                            serde_json::json!({"success":true,"count":0,"data":[],"continue_token":"next"}),
                        ),
                        "duplicate" => ResponseTemplate::new(200).set_body_json(
                            serde_json::json!({"success":true,"count":1,"data":[{"metadata":{"name":"one"}}]}),
                        ),
                        "count" => ResponseTemplate::new(200)
                            .set_body_json(serde_json::json!({"success":true,"count":1,"data":[]})),
                        _ => unreachable!(),
                    }
                })
                .expect(2)
                .mount(&server)
                .await;
            let error = test_client(server.uri(), None)
                .fetch_from_controller("/resources")
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{case}: {error}");
        }
    }

    #[tokio::test]
    async fn controller_scan_enforces_remaining_byte_budget() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("0123456789"))
            .mount(&server)
            .await;
        let error = test_client(server.uri(), None)
            .fetch_controller_page("/resources", None, &mut 9)
            .await
            .unwrap_err();
        assert!(error.contains("byte budget"));
    }

    #[tokio::test]
    async fn controller_fetch_sends_the_configured_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/HTTPRoute"))
            .and(header("authorization", "Bearer correct-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true,
                "data": [],
                "count": 0,
                "error": null
            })))
            .mount(&server)
            .await;
        let client = test_client(server.uri(), Some("correct-token".to_string()));

        let resources = client
            .fetch_from_controller("/api/v1/namespaced/HTTPRoute")
            .await
            .expect("the configured bearer token should authorize the request");

        assert!(resources.is_empty());
    }

    #[tokio::test]
    async fn controller_fetch_fails_when_the_bearer_token_is_wrong() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/HTTPRoute"))
            .and(header("authorization", "Bearer wrong-token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = test_client(server.uri(), Some("wrong-token".to_string()));

        let error = client
            .fetch_from_controller("/api/v1/namespaced/HTTPRoute")
            .await
            .expect_err("an unauthorized resource read must fail");

        assert!(error.contains("HTTP error: 401 Unauthorized"));
    }

    #[tokio::test]
    async fn resource_comparison_propagates_controller_read_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/namespaced/HTTPRoute"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = test_client(server.uri(), Some("wrong-token".to_string()));

        let error = compare_resources(&client, &[])
            .await
            .expect_err("an incomplete comparison must fail");

        assert!(error.contains("Failed to fetch HTTPRoute from controller"));
        assert!(error.contains("HTTP error: 401 Unauthorized"));
    }

    #[tokio::test]
    async fn secret_validation_lists_gateways_through_the_namespaced_api() {
        let server = MockServer::start().await;
        for endpoint in [
            "/api/v1/namespaced/Secret",
            "/api/v1/namespaced/EdgionTls",
            "/api/v1/namespaced/Gateway",
        ] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success": true,
                    "data": [],
                    "count": 0,
                    "error": null
                })))
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = test_client(server.uri(), None);

        let issues = validate_secret_references(&client)
            .await
            .expect("all required list reads should use valid endpoints");

        assert!(issues.is_empty());
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
