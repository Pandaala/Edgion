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
mod tests {
    use std::io::Write;

    use clap::Parser;
    use edgion_resources::resources::edgion_plugins::WafConfig;

    use super::*;

    #[test]
    fn inspect_is_local_and_text_output_is_value_free() {
        let cli = crate::cli::Cli::try_parse_from([
            "edgion-cli",
            "waf",
            "inspect",
            "--file",
            "rules.yaml",
            "--format",
            "text",
        ])
        .unwrap();
        assert!(matches!(cli.command, crate::cli::Commands::Waf(_)));
        let report = WafConfig {
            rules: "SecRule RESPONSE_BODY \"@rx secret-pattern\" \"id:2,phase:4,pass\"\n".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        let rendered = render_text(&report);
        assert!(rendered.contains("strictValidationPassed: false"));
        assert!(rendered.contains("full_response_body"));
        assert!(!rendered.contains("secret-pattern"));
    }

    #[test]
    fn inspect_reads_one_resource_and_returns_strict_result() {
        let mut valid = tempfile::NamedTempFile::new().unwrap();
        write!(valid, "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: example\nspec:\n  waf:\n    rules: 'SecRule REQUEST_URI \"@contains marker\" \"id:1,phase:1,pass\"'\n").unwrap();
        assert!(execute(&WafArgs {
            command: WafCommand::Inspect {
                file: valid.path().to_path_buf(),
                format: WafReportFormat::Json
            },
        })
        .is_ok());

        let mut rejected = tempfile::NamedTempFile::new().unwrap();
        write!(rejected, "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: example\nspec:\n  waf:\n    rules: 'SecRule RESPONSE_BODY \"@contains secret-marker\" \"id:2,phase:4,pass\"'\n").unwrap();
        assert!(execute(&WafArgs {
            command: WafCommand::Inspect {
                file: rejected.path().to_path_buf(),
                format: WafReportFormat::Json
            },
        })
        .is_err());

        let mut malformed = tempfile::NamedTempFile::new().unwrap();
        write!(
            malformed,
            "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nspec: [secret-marker"
        )
        .unwrap();
        let error = execute(&WafArgs {
            command: WafCommand::Inspect {
                file: malformed.path().to_path_buf(),
                format: WafReportFormat::Json,
            },
        })
        .unwrap_err();
        assert!(!error.to_string().contains("secret-marker"));
    }
}
