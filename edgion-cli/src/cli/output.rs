use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use tabled::{settings::Style, Table, Tabled};

/// Format a `creationTimestamp` as a kubectl-style AGE string.
///
/// Boundary semantics ([lo, hi) left-closed right-open):
///   [0, 60s)    -> `Ns`
///   [60s, 60m)  -> `Nm`  (exactly 60s renders as `1m`)
///   [60m, 24h)  -> `Nh`
///   [24h, ..)   -> `Nd`
///   None / negative duration / parse failure -> `<unknown>`
pub fn format_age(creation: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(ts) = creation else {
        return "<unknown>".to_string();
    };
    let dur = now.signed_duration_since(ts);
    let secs = dur.num_seconds();
    if secs < 0 {
        return "<unknown>".to_string();
    }
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    format!("{}d", days)
}

/// Format labels as `key=value,key=value` sorted by key. Empty map -> `<none>`.
///
/// BTreeMap iteration order is naturally lexicographic by key, so we lean on
/// that for deterministic output.
pub fn format_labels(labels: &BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return "<none>".to_string();
    }
    labels
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",")
}

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Wide,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Result<Self> {
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
    #[tabled(rename = "AGE")]
    pub age: String,
}

/// Wide row for table display with additional fields
#[derive(Tabled)]
pub struct ResourceRowWide {
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "NAMESPACE")]
    pub namespace: String,
    #[tabled(rename = "KIND")]
    pub kind: String,
    #[tabled(rename = "AGE")]
    pub age: String,
    #[tabled(rename = "LABELS")]
    pub labels: String,
    #[tabled(rename = "RESOURCE_VERSION")]
    pub resource_version: String,
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
        OutputFormat::Table => {
            let Some(items) = data.get("data").and_then(|d| d.as_array()) else {
                println!("{}", serde_json::to_string_pretty(data)?);
                return Ok(());
            };
            if items.is_empty() {
                println!("No resources found.");
                return Ok(());
            }
            let now = Utc::now();
            let rows: Vec<ResourceRow> = items
                .iter()
                .map(|item| ResourceRow {
                    name: extract_name(item).unwrap_or_else(|| "unknown".to_string()),
                    namespace: extract_namespace(item).unwrap_or_default(),
                    kind: extract_kind(item).unwrap_or_else(|| "unknown".to_string()),
                    age: format_age(extract_creation_timestamp(item), now),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::modern());
            println!("{}", table);
        }
        OutputFormat::Wide => {
            let Some(items) = data.get("data").and_then(|d| d.as_array()) else {
                println!("{}", serde_json::to_string_pretty(data)?);
                return Ok(());
            };
            if items.is_empty() {
                println!("No resources found.");
                return Ok(());
            }
            let now = Utc::now();
            let rows: Vec<ResourceRowWide> = items
                .iter()
                .map(|item| ResourceRowWide {
                    name: extract_name(item).unwrap_or_else(|| "unknown".to_string()),
                    namespace: extract_namespace(item).unwrap_or_default(),
                    kind: extract_kind(item).unwrap_or_else(|| "unknown".to_string()),
                    age: format_age(extract_creation_timestamp(item), now),
                    labels: format_labels(&extract_labels(item)),
                    resource_version: extract_resource_version(item),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::modern());
            println!("{}", table);
        }
    }
    Ok(())
}

/// Print resources accumulated across multiple pages (envelope-stripped).
///
/// `items` is the concatenated `data` arrays from each page's `ListResponse`.
/// Output shape:
///   - Json -> JSON array of items (pretty-printed)
///   - Yaml -> YAML sequence of items
///   - Table / Wide -> safety net: re-wrap into a synthesized ListResponse
///     and delegate to `print_resource_list`. The expected use of this
///     function is yaml/json only; callers stream per-page for table modes.
pub fn print_resource_list_all(items: Vec<Value>, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(&items)?);
        }
        OutputFormat::Table | OutputFormat::Wide => {
            let n = items.len();
            let synthesized = serde_json::json!({
                "success": true,
                "data": items,
                "count": n,
                "continue_token": null,
            });
            print_resource_list(&synthesized, format)?;
        }
    }
    Ok(())
}

/// Print one page of resources in table mode, with the header suppressed on
/// follow-up pages. Returns the number of items printed.
///
/// Used by the --all loop for table/wide modes; yaml/json modes accumulate
/// and use `print_resource_list_all` instead.
///
/// Table streaming header suppression is best-effort: tabled does not expose
/// a "no-header" toggle, so on follow-up pages we strip the top 3 lines
/// (border + header + separator) and the bottom 1 line (bottom border).
// FIXME(cli): pretty cross-page table streaming would need a custom Style.
pub fn print_resource_list_page_streaming(data: &Value, format: OutputFormat, suppress_header: bool) -> Result<usize> {
    let items = data
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let n = items.len();
    if n == 0 {
        return Ok(0);
    }
    match format {
        OutputFormat::Table => {
            let now = Utc::now();
            let rows: Vec<ResourceRow> = items
                .iter()
                .map(|item| ResourceRow {
                    name: extract_name(item).unwrap_or_else(|| "unknown".to_string()),
                    namespace: extract_namespace(item).unwrap_or_default(),
                    kind: extract_kind(item).unwrap_or_else(|| "unknown".to_string()),
                    age: format_age(extract_creation_timestamp(item), now),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::modern());
            let rendered = format!("{}", table);
            if suppress_header {
                // Skip top border + header + separator (3 lines)
                let lines: Vec<&str> = rendered.lines().collect();
                if lines.len() > 4 {
                    println!("{}", lines[3..].join("\n"));
                } else {
                    println!("{}", rendered);
                }
            } else {
                println!("{}", rendered);
            }
        }
        OutputFormat::Wide => {
            let now = Utc::now();
            let rows: Vec<ResourceRowWide> = items
                .iter()
                .map(|item| ResourceRowWide {
                    name: extract_name(item).unwrap_or_else(|| "unknown".to_string()),
                    namespace: extract_namespace(item).unwrap_or_default(),
                    kind: extract_kind(item).unwrap_or_else(|| "unknown".to_string()),
                    age: format_age(extract_creation_timestamp(item), now),
                    labels: format_labels(&extract_labels(item)),
                    resource_version: extract_resource_version(item),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::modern());
            let rendered = format!("{}", table);
            if suppress_header {
                let lines: Vec<&str> = rendered.lines().collect();
                if lines.len() > 4 {
                    println!("{}", lines[3..].join("\n"));
                } else {
                    println!("{}", rendered);
                }
            } else {
                println!("{}", rendered);
            }
        }
        OutputFormat::Json | OutputFormat::Yaml => {
            // Safety net -- caller should not use this for Json/Yaml in --all mode.
            print_resource_list(data, format)?;
        }
    }
    Ok(n)
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

/// Print error message to stderr
pub fn print_error(error: &str) {
    eprintln!("Error: {}", error);
}

/// Print success message with a checkmark indicator.
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

/// Extract `metadata.creationTimestamp` (RFC3339) from a resource JSON Value.
fn extract_creation_timestamp(resource: &Value) -> Option<DateTime<Utc>> {
    let ts_str = resource
        .get("metadata")
        .and_then(|m| m.get("creationTimestamp"))
        .and_then(|t| t.as_str())?;
    DateTime::parse_from_rfc3339(ts_str)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Extract `metadata.labels` as a BTreeMap (empty if absent or non-object).
fn extract_labels(resource: &Value) -> BTreeMap<String, String> {
    let Some(obj) = resource
        .get("metadata")
        .and_then(|m| m.get("labels"))
        .and_then(|l| l.as_object())
    else {
        return BTreeMap::new();
    };
    obj.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

/// Extract `metadata.resourceVersion` (string). Returns empty string on absence.
fn extract_resource_version(resource: &Value) -> String {
    resource
        .get("metadata")
        .and_then(|m| m.get("resourceVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
#[path = "../../tests/unit/cli/output_test.rs"]
mod tests;
