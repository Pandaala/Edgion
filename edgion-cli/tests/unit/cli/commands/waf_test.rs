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
    let report =
        WafConfig::inspect_inline_source("SecRule RESPONSE_BODY \"@rx secret-pattern\" \"id:2,phase:4,pass\"\n");
    let rendered = render_text(&report);
    assert!(rendered.contains("strictValidationPassed: false"));
    assert!(rendered.contains("full_response_body"));
    assert!(!rendered.contains("secret-pattern"));
}

#[test]
fn inspect_reads_one_rule_source_and_returns_strict_result() {
    let mut valid = tempfile::NamedTempFile::new().unwrap();
    writeln!(valid, "SecRule REQUEST_URI \"@contains marker\" \"id:1,phase:1,pass\"").unwrap();
    assert!(execute(&WafArgs {
        command: WafCommand::Inspect {
            file: valid.path().to_path_buf(),
            format: WafReportFormat::Json
        },
    })
    .is_ok());

    let mut rejected = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        rejected,
        "SecRule RESPONSE_BODY \"@contains secret-marker\" \"id:2,phase:4,pass\""
    )
    .unwrap();
    assert!(execute(&WafArgs {
        command: WafCommand::Inspect {
            file: rejected.path().to_path_buf(),
            format: WafReportFormat::Json
        },
    })
    .is_err());

    let mut malformed = tempfile::NamedTempFile::new().unwrap();
    write!(malformed, "SecRule invalid-secret-marker-(").unwrap();
    let error = execute(&WafArgs {
        command: WafCommand::Inspect {
            file: malformed.path().to_path_buf(),
            format: WafReportFormat::Json,
        },
    })
    .unwrap_err();
    assert!(!error.to_string().contains("secret-marker"));
}
