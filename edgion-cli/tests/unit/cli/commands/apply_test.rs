use super::*;
use crate::cli::{Cli, Commands};
use clap::Parser;

// ---- split_yaml_docs: pure function tests ----

#[test]
fn split_yaml_docs_single_document_returns_one() {
    let input = "kind: HTTPRoute\nmetadata:\n  name: foo\n";
    let docs = split_yaml_docs(input);
    assert_eq!(docs.len(), 1);
    assert!(docs[0].contains("kind: HTTPRoute"));
}

#[test]
fn split_yaml_docs_separator_splits_into_two() {
    let input = "kind: A\n---\nkind: B\n";
    let docs = split_yaml_docs(input);
    assert_eq!(docs.len(), 2);
    assert!(docs[0].contains("kind: A"));
    assert!(docs[1].contains("kind: B"));
}

#[test]
fn split_yaml_docs_skips_empty_documents() {
    // Leading separator yields no document before it; same for empty middle docs.
    let input = "---\nkind: A\n---\n---\nkind: B\n";
    let docs = split_yaml_docs(input);
    assert_eq!(docs.len(), 2);
}

#[test]
fn split_yaml_docs_empty_input_returns_empty() {
    assert!(split_yaml_docs("").is_empty());
    assert!(split_yaml_docs("   \n   \n").is_empty());
}

#[test]
fn split_yaml_docs_ignores_dash_inside_value() {
    // A dash sequence that is part of a value (has trailing content after dashes)
    // must not be treated as a document separator.
    let input = "kind: A\nkey: --not-a-separator\nmore: data\n";
    let docs = split_yaml_docs(input);
    assert_eq!(docs.len(), 1);
    assert!(docs[0].contains("--not-a-separator"));
}

// ---- Cli parsing tests for the Apply subcommand ----

#[test]
fn cli_parses_apply_with_required_file_flag() {
    let cli =
        Cli::try_parse_from(["edgion-cli", "apply", "--file", "config.yaml"]).expect("apply with --file should parse");
    match cli.command {
        Commands::Apply { file, dry_run } => {
            assert_eq!(file, vec!["config.yaml"]);
            assert!(!dry_run);
        }
        other => panic!("expected Apply, got {:?}", other),
    }
}

#[test]
fn cli_parses_repeated_apply_file_flags_in_order() {
    let cli = Cli::try_parse_from(["edgion-cli", "apply", "--file", "base.yaml", "--file", "route.yaml"])
        .expect("apply with repeated --file should parse");
    match cli.command {
        Commands::Apply { file, dry_run } => {
            assert_eq!(file, vec!["base.yaml", "route.yaml"]);
            assert!(!dry_run);
        }
        other => panic!("expected Apply, got {:?}", other),
    }
}

#[test]
fn cli_parses_apply_with_dry_run_flag() {
    let cli = Cli::try_parse_from(["edgion-cli", "apply", "-f", "config.yaml", "--dry-run"])
        .expect("apply --dry-run should parse");
    match cli.command {
        Commands::Apply { dry_run, .. } => assert!(dry_run),
        _ => panic!("expected Apply"),
    }
}

#[test]
fn cli_apply_without_file_flag_fails() {
    let err = Cli::try_parse_from(["edgion-cli", "apply"]).expect_err("apply without --file should fail");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}
