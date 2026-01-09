//! Configuration Load Validator
//!
//! Validates that all YAML configuration files in examples/conf/ have been
//! successfully loaded by the edgion-controller without parsing errors.
//!
//! This tool runs before resource_diff in the integration test pipeline to
//! catch configuration issues early.

use clap::Parser;
use colored::Colorize;
use reqwest::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(name = "config_load_validator")]
#[command(about = "Validate that all YAML configs are loaded by controller")]
struct Args {
    /// Controller admin API URL
    #[arg(long, default_value = "http://127.0.0.1:5800")]
    controller_url: String,

    /// Config directory to validate
    #[arg(long, default_value = "examples/conf")]
    config_dir: String,

    /// Request timeout in seconds
    #[arg(long, default_value = "5")]
    timeout: u64,

    /// Maximum retry attempts for failed requests
    #[arg(long, default_value = "3")]
    max_retries: u32,
}

/// Configuration file metadata
#[derive(Debug, Clone)]
struct ConfigFile {
    path: PathBuf,
    kind: String,
    name: String,
    namespace: Option<String>,
    skip_validation: bool,
}

impl ConfigFile {
    fn display_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{} {}/{}", self.kind, ns, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }

    #[allow(dead_code)]
    fn resource_key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}/{}", ns, self.name),
            None => self.name.clone(),
        }
    }
}

/// Validation result
#[derive(Debug, Default)]
struct ValidationResult {
    total_files: usize,
    loaded: Vec<String>,
    not_loaded: Vec<(String, String)>, // (display_name, file_path)
    skipped: Vec<String>,
    parse_errors: Vec<(String, String)>, // (file_path, error_message)
}

impl ValidationResult {
    fn has_failures(&self) -> bool {
        !self.not_loaded.is_empty() || !self.parse_errors.is_empty()
    }
}

/// Controller API list response
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
}

#[derive(Deserialize, Debug)]
struct Metadata {
    name: String,
    #[allow(dead_code)]
    namespace: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    annotations: HashMap<String, String>,
}

/// Admin API client
struct AdminClient {
    client: Client,
    controller_url: String,
    max_retries: u32,
}

impl AdminClient {
    fn new(controller_url: String, timeout: Duration, max_retries: u32) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            controller_url,
            max_retries,
        }
    }

    /// Check if controller is reachable
    async fn check_health(&self) -> bool {
        let url = format!("{}/health", self.controller_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Fetch resources from controller with retry
    async fn fetch_resources(&self, endpoint: &str) -> Result<Vec<Resource>, String> {
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
}

/// Cluster-scoped resource kinds
fn is_cluster_scoped(kind: &str) -> bool {
    matches!(kind, "GatewayClass" | "Gateway" | "EdgionGatewayConfig")
}

/// Base conf resources that are not available via list/watch API
fn is_base_conf_resource(kind: &str) -> bool {
    matches!(kind, "GatewayClass" | "Gateway" | "EdgionGatewayConfig")
}

/// Get API endpoint for a resource kind
fn get_api_endpoint(kind: &str, namespace: Option<&str>) -> String {
    if is_cluster_scoped(kind) {
        format!("/api/v1/cluster/{}", kind)
    } else {
        match namespace {
            Some(ns) => format!("/api/v1/namespaced/{}/{}", kind, ns),
            None => format!("/api/v1/namespaced/{}", kind),
        }
    }
}

/// Scan YAML files in directory
fn scan_yaml_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut yaml_files = Vec::new();

    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "yaml" || ext == "yml" {
                    yaml_files.push(path);
                }
            }
        }
    }

    yaml_files.sort();
    Ok(yaml_files)
}

/// Extract metadata from YAML content
fn extract_metadata(path: &Path, content: &str) -> Result<ConfigFile, String> {
    // Handle multi-document YAML files (skip validation for now)
    if content.contains("\n---\n") || content.starts_with("---\n") {
        // Extract kind from first line of content if present
        let first_doc = content.split("\n---\n").next().unwrap_or(content);

        // Try to extract basic info for reporting
        let kind = first_doc
            .lines()
            .find(|line| line.starts_with("kind:"))
            .and_then(|line| line.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let name = first_doc
            .lines()
            .find(|line| line.trim().starts_with("name:"))
            .and_then(|line| line.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().to_string());

        // Multi-document YAML files are auto-skipped
        return Ok(ConfigFile {
            path: path.to_path_buf(),
            kind,
            name,
            namespace: None,
            skip_validation: true,
        });
    }

    // Parse YAML
    let value: serde_yaml::Value = serde_yaml::from_str(content).map_err(|e| format!("YAML parse error: {}", e))?;

    // Extract kind
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'kind' field")?
        .to_string();

    // Extract metadata
    let metadata = value.get("metadata").ok_or("Missing 'metadata' field")?;

    let name = metadata
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'metadata.name' field")?
        .to_string();

    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Check for skip annotation
    let skip_validation = metadata
        .get("annotations")
        .and_then(|a| a.get("edgion.io/skip-load-validation"))
        .and_then(|v| v.as_str())
        .map(|s| s == "true")
        .unwrap_or(false);

    Ok(ConfigFile {
        path: path.to_path_buf(),
        kind,
        name,
        namespace,
        skip_validation,
    })
}

/// Validate configurations
async fn validate_configs(client: &AdminClient, config_files: Vec<ConfigFile>) -> ValidationResult {
    let mut result = ValidationResult {
        total_files: config_files.len(),
        ..Default::default()
    };

    // Group files by (kind, namespace) for batch querying
    let mut kind_groups: HashMap<(String, Option<String>), Vec<ConfigFile>> = HashMap::new();

    for file in config_files {
        if file.skip_validation {
            result.skipped.push(file.display_name());
            continue;
        }

        // Skip base_conf resources as they are not available via list/watch API
        if is_base_conf_resource(&file.kind) {
            result.skipped.push(format!(
                "{} (base_conf resource - not available via list API)",
                file.display_name()
            ));
            continue;
        }

        let key = (file.kind.clone(), file.namespace.clone());
        kind_groups.entry(key).or_default().push(file);
    }

    // Query each kind group
    for ((kind, namespace), files) in kind_groups {
        let endpoint = get_api_endpoint(&kind, namespace.as_deref());

        match client.fetch_resources(&endpoint).await {
            Ok(resources) => {
                // Build set of loaded resource names
                let loaded_names: HashSet<String> = resources.iter().map(|r| r.metadata.name.clone()).collect();

                // Check each file
                for file in files {
                    if loaded_names.contains(&file.name) {
                        result.loaded.push(file.display_name());
                    } else {
                        result
                            .not_loaded
                            .push((file.display_name(), file.path.display().to_string()));
                    }
                }
            }
            Err(e) => {
                // Mark files as not loaded with error details
                for file in files {
                    result
                        .not_loaded
                        .push((file.display_name(), file.path.display().to_string()));
                    // Log the API error for debugging
                    eprintln!("  API query failed for {}: {}", kind, e);
                }
            }
        }
    }

    result
}

/// Print validation report
fn print_report(result: &ValidationResult, controller_url: &str, config_dir: &str) {
    println!("\n{}", "=".repeat(60));
    println!("  {}", "Configuration Load Validation Report".bold());
    println!("{}\n", "=".repeat(60));

    println!("Controller:  {}", controller_url);
    println!("Config Dir:  {}\n", config_dir);

    // Loaded resources
    if !result.loaded.is_empty() {
        println!("{} {} resources loaded successfully:", "✓".green(), result.loaded.len());
        for name in &result.loaded {
            println!("  {} {}", "✓".green(), name);
        }
        println!();
    }

    // Skipped resources
    if !result.skipped.is_empty() {
        println!("{} {} resources skipped:", "⊘".yellow(), result.skipped.len());
        for name in &result.skipped {
            println!("  {} {} (skip-load-validation: true)", "⊘".yellow(), name);
        }
        println!();
    }

    // Not loaded resources
    if !result.not_loaded.is_empty() {
        println!("{} {} resources NOT loaded:", "✗".red(), result.not_loaded.len());
        for (name, path) in &result.not_loaded {
            println!("  {} {}", "✗".red(), name);
            println!("    File: {}", path.dimmed());
            println!("    Reason: Not found in controller");
        }
        println!();
    }

    // Parse errors
    if !result.parse_errors.is_empty() {
        println!("{} {} parse/query errors:", "✗".red(), result.parse_errors.len());
        for (path, error) in &result.parse_errors {
            println!("  {} {}", "✗".red(), path);
            println!("    Error: {}", error.dimmed());
        }
        println!();
    }

    // Summary
    println!("{}", "=".repeat(60));
    println!(
        "Summary: {}/{} loaded, {} skipped, {} failed",
        result.loaded.len(),
        result.total_files,
        result.skipped.len(),
        result.not_loaded.len() + result.parse_errors.len()
    );

    if result.has_failures() {
        println!("{}", "✗ Configuration validation FAILED".red().bold());
        println!(
            "\n{}",
            "Tip: Check controller logs for detailed parsing errors".yellow()
        );
    } else {
        println!("{}", "✓ All required configurations loaded successfully".green().bold());
    }

    println!("{}\n", "=".repeat(60));
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Create admin client
    let client = AdminClient::new(
        args.controller_url.clone(),
        Duration::from_secs(args.timeout),
        args.max_retries,
    );

    // Check controller connectivity
    println!("Checking controller connectivity...");
    if !client.check_health().await {
        eprintln!("{} Cannot connect to controller at {}", "✗".red(), args.controller_url);
        process::exit(2);
    }
    println!("{} Connected to controller\n", "✓".green());

    // Scan config files
    let config_path = Path::new(&args.config_dir);
    println!("Scanning config files in {}...", config_path.display());

    let yaml_files = match scan_yaml_files(config_path) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("{} {}", "✗".red(), e);
            process::exit(2);
        }
    };

    println!("Found {} YAML files\n", yaml_files.len());

    // Extract metadata from each file
    let mut config_files = Vec::new();
    let mut parse_errors = Vec::new();

    for path in yaml_files {
        match fs::read_to_string(&path) {
            Ok(content) => match extract_metadata(&path, &content) {
                Ok(config) => config_files.push(config),
                Err(e) => {
                    parse_errors.push((path.display().to_string(), e));
                }
            },
            Err(e) => {
                parse_errors.push((path.display().to_string(), format!("Read error: {}", e)));
            }
        }
    }

    // Validate configurations
    println!("Validating resources...\n");
    let mut result = validate_configs(&client, config_files).await;
    result.parse_errors.extend(parse_errors);

    // Print report
    print_report(&result, &args.controller_url, &args.config_dir);

    // Exit with appropriate code
    if result.has_failures() {
        process::exit(1);
    } else {
        process::exit(0);
    }
}
