use std::io::{Seek, Write};

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

#[test]
fn tags_cli_has_bounded_local_inputs_and_exclusive_queries() {
    let parse = |tail: &[&str]| {
        let mut args = vec!["edgion-cli", "waf", "tags", "--bundle", "a.yaml", "--bundle", "b.yaml"];
        args.extend_from_slice(tail);
        crate::cli::Cli::try_parse_from(args)
    };
    assert!(parse(&[
        "--selection",
        "selection.yaml",
        "--tag",
        "language-php",
        "--offset",
        "128"
    ])
    .is_ok());
    assert!(parse(&["--tag", "php", "--selected"]).is_err());
    assert!(parse(&["--selected", "--excluded"]).is_err());
    let mut input = tempfile::NamedTempFile::new().unwrap();
    input.write_all(b"12345").unwrap();
    assert_eq!(read_local_input(input.path(), 5).unwrap(), b"12345");
    assert!(read_local_input(input.path(), 4).is_err());
    assert!(read_local_input(Path::new("-"), 5).is_err());
}

#[test]
fn tags_reads_bundle_resource_and_selection_without_disclosing_source_errors() {
    let mut bundle = tempfile::NamedTempFile::new().unwrap();
    write!(bundle, "apiVersion: edgion.io/v1\nkind: EdgionConfigData\nmetadata: {{name: local}}\nspec:\n  data:\n    type: WafRuleBundle\n    config:\n      version: '1'\n      profile: local\n      provenance: test/revision\n      roots: [main.conf]\n      rules:\n        - name: main.conf\n          content: 'SecAction \"id:1,phase:1,pass,tag:php\"'\n").unwrap();
    let mut selection = tempfile::NamedTempFile::new().unwrap();
    selection.write_all(b"include: {tag: unknown}\n").unwrap();
    let args = WafArgs {
        command: WafCommand::Tags {
            bundle: vec![bundle.path().into()],
            selection: Some(selection.path().into()),
            tag: None,
            selected: true,
            excluded: false,
            offset: Some(0),
            format: WafReportFormat::Json,
        },
    };
    assert!(execute(&args).is_ok());
    selection.as_file_mut().set_len(0).unwrap();
    selection.as_file_mut().rewind().unwrap();
    selection
        .write_all(b"include: {tag: php, secret-key: private}\n")
        .unwrap();
    let error = execute(&args).unwrap_err().to_string();
    assert!(!error.contains("private"));
    assert!(!error.contains("secret-key"));
}

#[test]
fn local_bundle_envelope_is_required_and_checked_without_a_second_dom() {
    let document = "apiVersion: edgion.io/v1\nkind: EdgionConfigData\nmetadata: {name: local}\nspec:\n  data:\n    type: WafRuleBundle\n    config:\n      version: '1'\n      profile: local\n      provenance: test/revision\n      roots: [main.conf]\n      rules:\n        - name: main.conf\n          content: 'SecAction \"id:1,phase:1,pass\"'\n";
    assert!(decode_local_bundle(document.as_bytes()).is_ok());
    for invalid in [
        document.replace("kind: EdgionConfigData\n", ""),
        document.replace("apiVersion: edgion.io/v1\n", ""),
        document.replace("EdgionConfigData", "OtherKind"),
        document.replace("edgion.io/v1", "other.io/v1"),
        document.replace("metadata: {name: local}", "metadata: {}"),
    ] {
        assert!(decode_local_bundle(invalid.as_bytes()).is_err());
    }
}
