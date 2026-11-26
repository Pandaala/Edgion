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

async fn validation_server() -> (wiremock::MockServer, EdgionClient) {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/validate/Service"))
        .and(header("Authorization", "Bearer dry-run-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "data": {
                "ok": true, "unknownFieldCheck": "unsupported",
                "schemaErrors": [], "unknownFields": [], "validationErrors": []
            }
        })))
        .mount(&server)
        .await;
    let client = EdgionClient::with_static_bearer(
        crate::cli::TargetType::Center,
        Some(server.uri()),
        None,
        &crate::cli::client::ServerTrust::System,
        "dry-run-test",
    )
    .unwrap();
    (server, client)
}

#[tokio::test]
async fn dry_run_reports_controller_errors_for_the_failed_document_without_writing() {
    use wiremock::matchers::{body_string, method, path};
    use wiremock::{Mock, ResponseTemplate};
    let (server, client) = validation_server().await;
    let doc = "kind: EdgionPlugins\nmetadata:\n  name: invalid\nspec: {}\n";
    Mock::given(method("POST"))
        .and(path("/api/v1/validate/EdgionPlugins"))
        .and(body_string(doc))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "data": {"ok": false, "unknownFieldCheck": "performed",
                "schemaErrors": ["missing spec field"], "unknownFields": ["spec.typo"],
                "validationErrors": ["invalid plugin"]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("documents.yaml");
    std::fs::write(&file, format!("kind: Service\nmetadata:\n  name: good\n---\n{doc}")).unwrap();
    let error = apply_many(&client, &[file.display().to_string()], true)
        .await
        .unwrap_err()
        .to_string();
    for expected in [
        "doc 2/2",
        "schema error: missing spec field",
        "unknown field: spec.typo",
        "validation error: invalid plugin",
    ] {
        assert!(error.contains(expected), "{error}");
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|r| r.method == "POST" && r.url.path().starts_with("/api/v1/validate/")));
}

#[tokio::test]
async fn dry_run_requires_a_successful_request_and_a_recognized_verdict() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    for (status, body) in [
        (403, "denied"),
        (200, "{}"),
        (200, r#"{"success":true,"data":{"ok":true}}"#),
    ] {
        let (server, client) = validation_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/validate/Secret"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        assert!(apply_doc(
            &client,
            Path::new("secret.yaml"),
            "kind: Secret\nmetadata:\n  name: example\n",
            1,
            1,
            true
        )
        .await
        .is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn directory_dry_run_distinguishes_no_files_success_and_partial_failure() {
    let (_server, client) = validation_server().await;
    for (valid, invalid) in [(0, 0), (2, 0), (0, 2), (1, 1)] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "not yaml").unwrap();
        for index in 0..valid {
            std::fs::write(
                dir.path().join(format!("valid-{index}.yaml")),
                format!("apiVersion: v1\nkind: Service\nmetadata:\n  name: valid-{index}\n"),
            )
            .unwrap();
        }
        for index in 0..invalid {
            std::fs::write(dir.path().join(format!("invalid-{index}.yml")), "").unwrap();
        }
        let result = apply_directory(&client, dir.path(), true).await;
        if invalid == 0 {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().to_string().contains(&format!(
                "Failed to apply {invalid} of {} YAML file(s)",
                valid + invalid
            )));
        }
    }
}

#[tokio::test]
async fn batch_dry_run_propagates_directory_and_single_file_failures() {
    let (_server, client) = validation_server().await;
    let root = tempfile::tempdir().unwrap();
    let failed_dir = root.path().join("failed");
    std::fs::create_dir(&failed_dir).unwrap();
    let bad_file = failed_dir.join("bad.yaml");
    std::fs::write(&bad_file, "").unwrap();
    let good_file = root.path().join("later.yaml");
    std::fs::write(&good_file, "apiVersion: v1\nkind: Service\nmetadata:\n  name: later\n").unwrap();
    let paths = vec![
        failed_dir.to_string_lossy().into_owned(),
        good_file.to_string_lossy().into_owned(),
    ];
    let error = apply_many(&client, &paths, true).await.unwrap_err().to_string();
    assert!(error.contains("Failed to apply 1 of 2 input path(s)"));
    assert!(error.contains("Failed to apply 1 of 1 YAML file(s)"));
    let error = apply(&client, bad_file.to_str().unwrap(), true).await.unwrap_err();
    assert!(error.to_string().contains("Empty YAML file"));
}

// ---- create/update race: create 409 falls back to update ----

/// The existence probe reads the Controller's processed cache, which can trail
/// a write that just reached storage. A create that answers 409 Conflict must
/// fall back to an update instead of failing the apply.
#[tokio::test]
async fn create_conflict_falls_back_to_update() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/namespaced/secret/edgion-test"))
        .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/namespaced/secret/edgion-test/race"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;

    let file = std::env::temp_dir().join(format!("edgion-apply-race-{}.yaml", std::process::id()));
    std::fs::write(
        &file,
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: race\n  namespace: edgion-test\n",
    )
    .unwrap();
    let client = EdgionClient::new(
        crate::cli::TargetType::Center,
        Some(server.uri()),
        None,
        &crate::cli::client::ServerTrust::System,
    )
    .unwrap();
    let result = apply(&client, file.to_str().unwrap(), false).await;
    let _ = std::fs::remove_file(&file);
    assert!(result.is_ok(), "{result:?}");
}

/// A create rejected for any other reason still fails the apply.
#[tokio::test]
async fn create_validation_error_is_not_retried_as_update() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string("invalid"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let file = std::env::temp_dir().join(format!("edgion-apply-invalid-{}.yaml", std::process::id()));
    std::fs::write(
        &file,
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: bad\n  namespace: edgion-test\n",
    )
    .unwrap();
    let client = EdgionClient::new(
        crate::cli::TargetType::Center,
        Some(server.uri()),
        None,
        &crate::cli::client::ServerTrust::System,
    )
    .unwrap();
    let result = apply(&client, file.to_str().unwrap(), false).await;
    let _ = std::fs::remove_file(&file);
    assert!(result.is_err());
}

/// A follower Controller answers every write with the not-leader 409. That
/// is not an "already exists" answer: the apply reports it (with the leader
/// address) and never retries the write as an update.
#[tokio::test]
async fn not_leader_conflict_is_reported_and_not_retried_as_update() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let not_leader = serde_json::json!({"error": "not leader", "leader": "10.0.0.7:5800"});
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(409).set_body_json(not_leader.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(409).set_body_json(not_leader))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(0)
        .mount(&server)
        .await;

    let file = std::env::temp_dir().join(format!("edgion-apply-not-leader-{}.yaml", std::process::id()));
    std::fs::write(
        &file,
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: standby\n  namespace: edgion-test\n",
    )
    .unwrap();
    let client = EdgionClient::new(
        crate::cli::TargetType::Center,
        Some(server.uri()),
        None,
        &crate::cli::client::ServerTrust::System,
    )
    .unwrap();
    let result = apply(&client, file.to_str().unwrap(), false).await;
    let _ = std::fs::remove_file(&file);
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("not leader"), "{error}");
    assert!(error.contains("10.0.0.7:5800"), "{error}");
}

#[test]
fn not_leader_error_recognizes_only_the_not_leader_body() {
    assert_eq!(
        not_leader_error(r#"{"error":"not leader","leader":null}"#).as_deref(),
        Some("API error: not leader; the leader is currently unknown, retry later")
    );
    assert!(not_leader_error(r#"{"error":"not leader","leader":"h:1"}"#)
        .unwrap()
        .contains("h:1"));
    assert_eq!(not_leader_error("already exists"), None);
    assert_eq!(not_leader_error(r#"{"error":"AlreadyExists"}"#), None);
}
