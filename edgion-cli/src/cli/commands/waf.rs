//! Local inspection of one EdgionPlugins inline WAF configuration.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Args, Subcommand, ValueEnum};
use edgion_resources::resources::edgion_plugins::{EdgionPlugins, WafSupportReport};

const MAX_WAF_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Args)]
pub struct WafArgs {
    #[command(subcommand)]
    pub command: WafCommand,
}

#[derive(Debug, Subcommand)]
pub enum WafCommand {
    /// Inspect one EdgionPlugins document without publishing it.
    Inspect {
        #[arg(long, value_name = "EDGIONPLUGINS.YAML")]
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
        WafCommand::Inspect { file, format } => {
            let bytes = read_bounded(file)?;
            let document: EdgionPlugins = serde_yaml::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid EdgionPlugins YAML; rule source was not displayed"))?;
            let Some(waf) = document.spec.waf else {
                bail!("EdgionPlugins document has no spec.waf configuration");
            };
            let report = waf.inspect_support();
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

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        bail!("stdin is not supported; pass an EdgionPlugins file path");
    }
    let file = File::open(path).map_err(|_| anyhow::anyhow!("cannot open WAF document"))?;
    let mut bytes = Vec::new();
    file.take(MAX_WAF_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("cannot read WAF document"))?;
    if bytes.len() as u64 > MAX_WAF_DOCUMENT_BYTES {
        bail!("WAF document exceeds the 16 MiB inspection limit");
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
