use anyhow::Result;
use serde_json::Value;
use tabled::{settings::Style, Table, Tabled};

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Wide,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            "wide" => Ok(Self::Wide),
            _ => anyhow::bail!("Unknown output format: {}. Available: table, json, yaml, wide", s),
        }
    }
}

/// Simple row for table display
#[derive(Tabled)]
pub struct ResourceRow {
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "NAMESPACE")]
    pub namespace: String,
    #[tabled(rename = "KIND")]
    pub kind: String,
}

/// Print a list of resources
pub fn print_resource_list(data: &Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(data)?);
        }
        OutputFormat::Table | OutputFormat::Wide => {
            // Extract items from ListResponse format
            if let Some(items) = data.get("data").and_then(|d| d.as_array()) {
                if items.is_empty() {
                    println!("No resources found.");
                    return Ok(());
                }

                let mut rows = Vec::new();
                for item in items {
                    let name = extract_name(item).unwrap_or("unknown".to_string());
                    let namespace = extract_namespace(item).unwrap_or("".to_string());
                    let kind = extract_kind(item).unwrap_or("unknown".to_string());

                    rows.push(ResourceRow { name, namespace, kind });
                }

                let mut table = Table::new(rows);
                table.with(Style::modern());
                println!("{}", table);
            } else {
                // Fallback to JSON if not in expected format
                println!("{}", serde_json::to_string_pretty(data)?);
            }
        }
    }
    Ok(())
}

/// Print a single resource
pub fn print_resource(data: &Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(data)?);
        }
        _ => {
            // For table/wide, show as YAML for single resource
            println!("{}", serde_yaml::to_string(data)?);
        }
    }
    Ok(())
}

/// Print a simple message
pub fn print_message(message: &str) {
    println!("{}", message);
}

/// Print error message to stderr
pub fn print_error(error: &str) {
    eprintln!("Error: {}", error);
}

/// Print success message in green (if terminal supports colors)
pub fn print_success(message: &str) {
    println!("✓ {}", message);
}

/// Extract name from resource metadata
fn extract_name(resource: &Value) -> Option<String> {
    resource
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Extract namespace from resource metadata
fn extract_namespace(resource: &Value) -> Option<String> {
    resource
        .get("metadata")
        .and_then(|m| m.get("namespace"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Extract kind from resource
fn extract_kind(resource: &Value) -> Option<String> {
    resource.get("kind").and_then(|k| k.as_str()).map(|s| s.to_string())
}
