//! Local inspection of one SecLang rule source.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Args, Subcommand, ValueEnum};
use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
use edgion_resources::resources::edgion_config_data::{EdgionConfigData, WafRuleSelection, WafTagQuery};
use edgion_resources::resources::edgion_plugins::{WafConfig, WafSupportReport};
use kube::api::ObjectMeta;
use serde::Deserialize;

const MAX_WAF_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

/// kube's resource decoder treats input kind/apiVersion as unknown; local inspection checks them explicitly.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalBundleEnvelope {
    api_version: String,
    kind: String,
    metadata: ObjectMeta,
    spec: EdgionConfigDataSpec,
    #[serde(default, rename = "status")]
    _status: Option<serde::de::IgnoredAny>,
}

fn decode_local_bundle(bytes: &[u8]) -> Result<EdgionConfigData> {
    let input: LocalBundleEnvelope =
        serde_yaml::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid local WafRuleBundle resource"))?;
    let name = input
        .metadata
        .name
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("local WAF bundle requires a resource name"))?;
    if input.api_version != "edgion.io/v1"
        || input.kind != "EdgionConfigData"
        || name.is_empty()
        || name.len() > 253
        || name.chars().any(char::is_control)
        || name.contains('/')
        || input.metadata.namespace.as_ref().is_some_and(|namespace| {
            namespace.is_empty()
                || namespace.len() > 63
                || namespace.chars().any(char::is_control)
                || namespace.contains('/')
        })
    {
        bail!("invalid local WAF resource envelope");
    }
    let mut resource = EdgionConfigData::new(name, input.spec);
    resource.metadata = input.metadata;
    Ok(resource)
}

#[derive(Debug, Args)]
pub struct WafArgs {
    #[command(subcommand)]
    pub command: WafCommand,
}

#[derive(Debug, Subcommand)]
pub enum WafCommand {
    /// Inspect tags and static selection in explicitly supplied local bundle resources.
    Tags {
        #[arg(long, required = true, value_name = "BUNDLE.YAML", num_args = 1)]
        bundle: Vec<PathBuf>,
        #[arg(long, value_name = "SELECTION.YAML")]
        selection: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["selected", "excluded"])]
        tag: Option<String>,
        #[arg(long, conflicts_with = "excluded")]
        selected: bool,
        #[arg(long)]
        excluded: bool,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long, value_enum, default_value = "text")]
        format: WafReportFormat,
    },
    /// Inspect one local SecLang source without publishing it.
    Inspect {
        #[arg(long, value_name = "RULES.CONF")]
        file: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        format: WafReportFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WafReportFormat {
    Text,
    Json,
}

pub fn execute(args: &WafArgs) -> Result<()> {
    match &args.command {
        WafCommand::Tags {
            bundle,
            selection,
            tag,
            selected,
            excluded,
            offset,
            format,
        } => {
            if bundle.len() > 16 {
                bail!("WAF tags requires between 1 and 16 local bundles");
            }
            if offset.is_some() && tag.is_none() && !selected && !excluded {
                bail!("--offset requires --tag, --selected or --excluded");
            }
            if tag
                .as_ref()
                .is_some_and(|tag| tag.len() > 256 || tag.trim().is_empty() || tag.chars().any(char::is_control))
            {
                bail!("--tag requires a nonempty literal tag of at most 256 bytes without control characters");
            }
            let mut remaining = 64 * 1024 * 1024;
            // Read all serialized inputs under one cap before decoding any document.
            let mut inputs = Vec::new();
            inputs
                .try_reserve_exact(bundle.len())
                .map_err(|_| anyhow::anyhow!("cannot allocate WAF bundle inputs"))?;
            for path in bundle {
                let bytes = read_local_input(path, remaining)?;
                remaining -= bytes.len();
                inputs.push(bytes);
            }
            let selection_bytes = match selection {
                Some(path) => Some(read_local_input(path, remaining.min(64 * 1024))?),
                None => None,
            };
            let selection = match &selection_bytes {
                Some(bytes) => serde_yaml::from_slice::<WafRuleSelection>(bytes)
                    .map_err(|_| anyhow::anyhow!("invalid bounded WAF rule selection"))?,
                None => WafRuleSelection::default(),
            };
            let resources: Vec<EdgionConfigData> = inputs
                .iter()
                .map(|bytes| decode_local_bundle(bytes))
                .collect::<Result<_>>()?;
            let query = if let Some(tag) = tag {
                WafTagQuery::Tag(tag)
            } else if *selected {
                WafTagQuery::Selected
            } else if *excluded {
                WafTagQuery::Excluded
            } else {
                WafTagQuery::Catalog
            };
            let report = WafConfig::inspect_local_bundles(&resources, &selection, query, offset.unwrap_or(0))
                .map_err(|_| anyhow::anyhow!("cannot inspect local WAF bundles"))?;
            let encoded = report.to_json().map_err(anyhow::Error::msg)?;
            match format {
                WafReportFormat::Json => println!("{encoded}"),
                // The escaped compact document keeps text output bounded and safe for terminals.
                WafReportFormat::Text => println!("WAF local metadata\n{encoded}"),
            }
            if report.failure.is_some() {
                bail!("WAF local bundles did not pass inspection; see the report above");
            }
            Ok(())
        }
        WafCommand::Inspect { file, format } => {
            let bytes = read_bounded(file)?;
            let source = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("WAF rule source is not UTF-8; source was not displayed"))?;
            let report = WafConfig::inspect_inline_source(source);
            match format {
                WafReportFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
                WafReportFormat::Text => print!("{}", render_text(&report)),
            }
            if !report.strict_validation_passed {
                bail!("WAF rules did not pass strict validation; see the report above");
            }
            Ok(())
        }
    }
}

fn read_local_input(path: &Path, cap: usize) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        bail!("stdin is not supported; pass a local resource file path");
    }
    let mut file = File::open(path).map_err(|_| anyhow::anyhow!("cannot open local WAF input"))?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let remaining = cap.saturating_sub(bytes.len());
        let read_size = chunk.len().min(remaining.saturating_add(1));
        let size = file
            .read(&mut chunk[..read_size])
            .map_err(|_| anyhow::anyhow!("cannot read local WAF input"))?;
        if size == 0 {
            return Ok(bytes);
        }
        if size > remaining {
            bail!("local WAF input exceeds the serialized byte limit");
        }
        bytes
            .try_reserve_exact(size)
            .map_err(|_| anyhow::anyhow!("cannot allocate local WAF input"))?;
        bytes.extend_from_slice(&chunk[..size]);
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        bail!("stdin is not supported; pass a rule source file path");
    }
    let file = File::open(path).map_err(|_| anyhow::anyhow!("cannot open WAF rule source"))?;
    let mut bytes = Vec::new();
    file.take(MAX_WAF_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("cannot read WAF rule source"))?;
    if bytes.len() as u64 > MAX_WAF_SOURCE_BYTES {
        bail!("WAF rule source exceeds the 16 MiB inspection limit");
    }
    Ok(bytes)
}

fn render_text(report: &WafSupportReport) -> String {
    let mut output = format!(
        "reportVersion: {}\nanalysisComplete: {}\nstrictValidationPassed: {}\nruleCount: {}\nunavailableRuleCount: {}\nunattributedFindings: {}\nfindings: {}\nomittedFindings: {}\nomittedRuleSummaries: {}\n",
        report.report_version,
        report.analysis_complete,
        report.strict_validation_passed,
        report.rule_count.map_or("unknown".to_string(), |value| value.to_string()),
        report.unavailable_rule_count.map_or("unknown".to_string(), |value| value.to_string()),
        report.unattributed_findings.map_or("unknown".to_string(), |value| value.to_string()),
        report.findings.len(),
        report.omitted_findings,
        report.omitted_rule_summaries,
    );
    for finding in &report.findings {
        let id = finding.rule_id.map_or("unknown".to_string(), |value| value.to_string());
        output.push_str(&format!(
            "rule {id} at document {} directive {}: {}: {}\n",
            finding.document_index, finding.directive_index, finding.capability, finding.reason
        ));
    }
    if let Some(failure) = report.failure {
        output.push_str(&format!("failure: {}: {}", failure.category, failure.reason));
        if let Some(line) = failure.physical_line {
            output.push_str(&format!(" on physical line {line}"));
        }
        if let (Some(document), Some(directive)) = (failure.document_index, failure.directive_index) {
            output.push_str(&format!(" at document {document} directive {directive}"));
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
#[path = "../../../tests/unit/cli/commands/waf_test.rs"]
mod tests;
