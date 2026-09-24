use crate::cli::{Cli, Commands};
use clap::Parser;

#[test]
fn cli_parses_delete_with_kind_and_name_positional() {
    let cli = Cli::try_parse_from(["edgion-cli", "delete", "httproute", "my-route"])
        .expect("delete with positional kind+name should parse");
    match cli.command {
        Commands::Delete {
            kind,
            name,
            namespace,
            file,
        } => {
            assert_eq!(kind.as_deref(), Some("httproute"));
            assert_eq!(name.as_deref(), Some("my-route"));
            assert!(namespace.is_none());
            assert!(file.is_none());
        }
        other => panic!("expected Delete, got {:?}", other),
    }
}

#[test]
fn cli_parses_delete_with_namespace_short_flag() {
    let cli = Cli::try_parse_from(["edgion-cli", "delete", "service", "svc-1", "-n", "edgion-system"])
        .expect("delete with -n should parse");
    match cli.command {
        Commands::Delete { namespace, .. } => {
            assert_eq!(namespace.as_deref(), Some("edgion-system"));
        }
        _ => panic!("expected Delete"),
    }
}

#[test]
fn cli_parses_delete_with_file_flag_only() {
    let cli = Cli::try_parse_from(["edgion-cli", "delete", "--file", "route.yaml"])
        .expect("delete with --file only should parse");
    match cli.command {
        Commands::Delete { kind, name, file, .. } => {
            assert!(kind.is_none());
            assert!(name.is_none());
            assert_eq!(file.as_deref(), Some("route.yaml"));
        }
        _ => panic!("expected Delete"),
    }
}

#[test]
fn cli_parses_delete_with_no_args_succeeds_at_parse_time() {
    // All positional args and --file are optional at the clap layer; the
    // mutual-exclusion check happens inside the runtime delete() function.
    // This documents that runtime invariant rather than enforcing it via clap.
    let cli = Cli::try_parse_from(["edgion-cli", "delete"]).expect("parse-only check succeeds");
    match cli.command {
        Commands::Delete {
            kind,
            name,
            namespace,
            file,
        } => {
            assert!(kind.is_none() && name.is_none() && namespace.is_none() && file.is_none());
        }
        _ => panic!("expected Delete"),
    }
}
