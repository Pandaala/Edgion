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

#[test]
fn list_page_namespace_selection_preserves_transport_pagination() {
    let source = json!({
        "success": true, "count": 3, "continue_token": "original-token",
        "data": [
            {"metadata": {"name": "first", "namespace": "team-a"}},
            {"metadata": {"name": "other", "namespace": "team-b"}},
            {"metadata": {"name": "last", "namespace": "team-a"}}
        ]
    });
    for target in [TargetType::Center, TargetType::Server, TargetType::Client] {
        for namespace in [None, Some("team-a"), Some("missing")] {
            let mut page = source.clone();
            filter_list_page(&mut page, target, namespace);
            assert_eq!(page["continue_token"], "original-token");
            assert_eq!(page["success"], true);
            if target == TargetType::Center || namespace.is_none() {
                assert_eq!(page, source);
            } else if namespace == Some("missing") {
                assert_eq!(page["data"], json!([]));
                assert_eq!(page["count"], 0);
            } else {
                assert_eq!(page["data"], json!([source["data"][0], source["data"][2]]));
                assert_eq!(page["count"], 2);
            }
        }
    }
}

#[test]
fn empty_filtered_page_keeps_token_for_later_matching_page() {
    for target in [TargetType::Server, TargetType::Client] {
        let mut empty = json!({
            "data": [{"metadata": {"namespace": "other"}}],
            "count": 1, "continue_token": "next-page"
        });
        filter_list_page(&mut empty, target, Some("wanted"));
        assert_eq!(empty["count"], 0);
        assert_eq!(empty["continue_token"], "next-page");
        for format in [OutputFormat::Table, OutputFormat::Wide] {
            assert_eq!(
                crate::cli::output::print_resource_list_page_streaming(&empty, format, false).unwrap(),
                0
            );
        }
        let mut next = json!({
            "data": [{"metadata": {"name": "visible", "namespace": "wanted"}}],
            "count": 1, "continue_token": null
        });
        filter_list_page(&mut next, target, Some("wanted"));
        assert_eq!(next["data"][0]["metadata"]["name"], "visible");
        assert_eq!(next["count"], 1);
        assert!(next["continue_token"].is_null());
    }
}

#[test]
fn cli_parses_all_cache_namespace_queries_for_each_output_format() {
    for (target, expected_target) in [("server", TargetType::Server), ("client", TargetType::Client)] {
        for format in ["table", "wide", "json", "yaml"] {
            let cli = Cli::try_parse_from([
                "edgion-cli",
                "-t",
                target,
                "get",
                "httproute",
                "-n",
                "team-a",
                "--all",
                "-o",
                format,
            ])
            .unwrap();
            assert_eq!(cli.target, expected_target);
            let Commands::Get {
                namespace, output, all, ..
            } = cli.command
            else {
                panic!("get expected")
            };
            assert_eq!(namespace.as_deref(), Some("team-a"));
            assert_eq!(output, format);
            assert!(all);
        }
    }
}
