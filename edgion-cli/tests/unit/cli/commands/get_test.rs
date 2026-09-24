use super::*;
use crate::cli::{Cli, Commands};
use clap::Parser;
use serde_json::json;

// ---- filter_by_namespace ----

#[test]
fn filter_by_namespace_keeps_only_matching_items_and_updates_count() {
    let mut data = json!({
        "data": [
            {"metadata": {"name": "a", "namespace": "ns1"}},
            {"metadata": {"name": "b", "namespace": "ns2"}},
            {"metadata": {"name": "c", "namespace": "ns1"}},
        ],
        "count": 3,
    });
    filter_by_namespace(&mut data, "ns1");
    let items = data["data"].as_array().expect("data should be array");
    assert_eq!(items.len(), 2);
    assert_eq!(data["count"].as_u64(), Some(2));
}

#[test]
fn filter_by_namespace_with_no_matches_yields_empty_array() {
    let mut data = json!({
        "data": [
            {"metadata": {"name": "a", "namespace": "other"}},
        ],
        "count": 1,
    });
    filter_by_namespace(&mut data, "missing");
    assert!(data["data"].as_array().unwrap().is_empty());
    assert_eq!(data["count"].as_u64(), Some(0));
}

#[test]
fn filter_by_namespace_is_no_op_when_data_is_not_array() {
    let mut data = json!({"data": "not-an-array", "count": 7});
    filter_by_namespace(&mut data, "ns1");
    // count must remain untouched when filter cannot run
    assert_eq!(data["count"].as_u64(), Some(7));
}

// ---- extract_resource_data ----

#[test]
fn extract_resource_data_center_returns_payload_unchanged() {
    let payload = json!({"kind": "HTTPRoute", "metadata": {"name": "x"}});
    let out = extract_resource_data(payload.clone(), TargetType::Center);
    assert_eq!(out, payload);
}

#[test]
fn extract_resource_data_server_unwraps_data_field() {
    let inner = json!({"kind": "HTTPRoute", "metadata": {"name": "x"}});
    let wrapped = json!({"success": true, "data": inner.clone()});
    let out = extract_resource_data(wrapped, TargetType::Server);
    assert_eq!(out, inner);
}

#[test]
fn extract_resource_data_client_returns_input_when_no_data_field() {
    let payload = json!({"foo": "bar"});
    let out = extract_resource_data(payload.clone(), TargetType::Client);
    assert_eq!(out, payload);
}

// ---- Cli parsing tests for the Get subcommand ----

#[test]
fn cli_parses_get_with_kind_only_lists_all_namespaces() {
    let cli = Cli::try_parse_from(["edgion-cli", "get", "httproute"]).expect("get <kind> should parse");
    match cli.command {
        Commands::Get {
            kind,
            name,
            namespace,
            output,
            metadata_only,
            limit,
            continue_token,
            all,
        } => {
            assert_eq!(kind, "httproute");
            assert!(name.is_none());
            assert!(namespace.is_none());
            assert_eq!(output, "table");
            assert!(!metadata_only);
            assert!(limit.is_none());
            assert!(continue_token.is_none());
            assert!(!all);
        }
        _ => panic!("expected Get"),
    }
}

#[test]
fn cli_parses_get_with_name_namespace_and_output() {
    let cli = Cli::try_parse_from([
        "edgion-cli",
        "get",
        "httproute",
        "my-route",
        "-n",
        "default",
        "-o",
        "yaml",
    ])
    .expect("get with all flags should parse");
    match cli.command {
        Commands::Get {
            kind,
            name,
            namespace,
            output,
            metadata_only,
            limit,
            continue_token,
            all,
        } => {
            assert_eq!(kind, "httproute");
            assert_eq!(name.as_deref(), Some("my-route"));
            assert_eq!(namespace.as_deref(), Some("default"));
            assert_eq!(output, "yaml");
            assert!(!metadata_only);
            assert!(limit.is_none());
            assert!(continue_token.is_none());
            assert!(!all);
        }
        _ => panic!("expected Get"),
    }
}

#[test]
fn cli_get_without_kind_fails() {
    let err = Cli::try_parse_from(["edgion-cli", "get"]).expect_err("get without kind should fail");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}
