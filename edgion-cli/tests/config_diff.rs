use serde_json::json;
use std::process::{Command, Output};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "apiVersion: v1\nkind: Secret\nmetadata: {name: sample}\n";
const SECRET_PATH: &str = "/api/v1/namespaced/Secret/default/sample";

async fn run(url: &str, files: &[&str], flags: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_edgion-cli"));
    command
        .env("HOME", dir.path())
        .env_remove("EDGION_TOKEN")
        .env_remove("EDGION_CA_CERT")
        .env("NO_PROXY", "*")
        .args(["--server", url])
        .args(flags)
        .arg("diff");
    for (i, content) in files.iter().enumerate() {
        let file = dir.path().join(format!("private-fixture-{i}.yaml"));
        std::fs::write(&file, content).unwrap();
        command.arg("-f").arg(file);
    }
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

async fn source(server: &MockServer, endpoint: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(header("authorization", "Bearer diff-token"))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

async fn assert_reads(server: &MockServer, count: usize) {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), count);
    for request in requests {
        assert_eq!(request.method.as_str(), "GET");
        assert!(request.body.is_empty());
    }
}

fn assert_report(output: Output, code: i32, report: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), report);
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn assert_failure(output: Output) -> String {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(!diagnostic.is_empty());
    assert!(!diagnostic.contains("private-"), "{diagnostic}");
    diagnostic
}

#[tokio::test]
async fn compares_structured_documents_in_input_order_without_writing() {
    let server = MockServer::start().await;
    let manifests = r#"apiVersion: v1
kind: Secret
metadata:
  name: sample
  labels: {z: last, a: first}
stringData:
  private-key: |
    first
    ---
    last
--- # second document
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata: {name: absent}
"#;
    let remote = json!({
        "apiVersion":"v1", "kind":"Secret",
        "metadata":{"name":"sample","namespace":"default","labels":{"a":"first","z":"last"},
            "resourceVersion":"17","managedFields":[{}],"uid":"generated","generation":2,
            "creationTimestamp":"2026-01-01T00:00:00Z"},
        "stringData":{"private-key":"first\n---\nlast\n"}, "status":{"phase":"private-status"}
    });
    source(&server, SECRET_PATH, ResponseTemplate::new(200).set_body_json(remote)).await;
    source(
        &server,
        "/api/v1/cluster/GatewayClass/absent",
        ResponseTemplate::new(404).insert_header("x-edgion-source-not-found", "true"),
    )
    .await;
    let plugin = json!({
        "apiVersion":"edgion.io/v1","kind":"EdgionPlugins",
        "metadata":{"name":"filters","namespace":"tenant","labels":{"private-label":"private-value"},
            "annotations":{"private-annotation":"private-value"},"finalizers":["private-finalizer"]},
        "spec":{"requestPlugins":[{"config":{"private-inline-key":"private-inline-value"}}]}
    });
    let mut persisted = plugin.clone();
    persisted["metadata"]
        .as_object_mut()
        .unwrap()
        .retain(|k, _| k == "name" || k == "namespace");
    persisted["spec"]["requestPlugins"] = json!([]);
    source(
        &server,
        "/api/v1/namespaced/EdgionPlugins/tenant/filters",
        ResponseTemplate::new(200).set_body_json(persisted),
    )
    .await;
    let output = run(
        &server.uri(),
        &[manifests, &plugin.to_string()],
        &["--token", "diff-token"],
    )
    .await;
    assert_report(
        output,
        1,
        "document 1: equal\ndocument 2: added\ndocument 3: changed (labels, annotations, metadata, content)\n",
    );
    assert_reads(&server, 3).await;
}

#[tokio::test]
async fn compares_plain_scalar_keys_using_their_original_spelling() {
    let server = MockServer::start().await;
    source(
        &server,
        "/api/v1/namespaced/ConfigMap/default/sample",
        ResponseTemplate::new(200).set_body_json(json!({
            "kind":"ConfigMap", "metadata":{"name":"sample","namespace":"default"},
            "data":{"8080":"upstream","true":"active","0x10":"hex","16":"decimal"}
        })),
    )
    .await;
    let input =
        "kind: ConfigMap\nmetadata: {name: sample}\ndata: {8080: upstream, true: active, 0x10: hex, 16: decimal}";
    assert_report(
        run(&server.uri(), &[input], &["--token", "diff-token"]).await,
        0,
        "document 1: equal\n",
    );
    let duplicate = "kind: ConfigMap\nmetadata: {name: sample}\ndata: {8080: first, '8080': second}";
    assert_failure(run(&server.uri(), &[duplicate], &["--token", "diff-token"]).await);
    assert_reads(&server, 1).await;
}

#[tokio::test]
async fn explicit_null_fails_the_batch_while_empty_documents_are_ignored() {
    let server = MockServer::start().await;
    let body = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"sample","namespace":"default"}});
    Mock::given(method("GET"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let input = format!("---\n# empty\n---\n{SECRET}---\n# trailing empty\n");
    assert_report(
        run(&server.uri(), &[&input], &["--token", "diff-token"]).await,
        0,
        "document 1: equal\n",
    );
    for invalid in ["null", "~", "{}", "''"] {
        assert_failure(
            run(
                &server.uri(),
                &[&format!("{SECRET}---\n{invalid}")],
                &["--token", "diff-token"],
            )
            .await,
        );
    }
    assert_reads(&server, 5).await;
}

#[tokio::test]
async fn compares_roundtrip_decimals_without_losing_a_one_ulp_change() {
    let manifest = "kind: EdgionPlugins\nmetadata: {name: sample}\nspec: {minimum: 51.248178375505404}";
    for (stored, code, expected) in [
        ("51.248178375505404", 0, "document 1: equal\n"),
        ("51.24817837550541", 1, "document 1: changed (content)\n"),
    ] {
        let server = MockServer::start().await;
        let body = format!(
            r#"{{"kind":"EdgionPlugins","metadata":{{"name":"sample","namespace":"default"}},"spec":{{"minimum":{stored}}}}}"#
        );
        source(
            &server,
            "/api/v1/namespaced/EdgionPlugins/default/sample",
            ResponseTemplate::new(200).set_body_raw(body, "application/json"),
        )
        .await;
        assert_report(
            run(&server.uri(), &[manifest], &["--token", "diff-token"]).await,
            code,
            expected,
        );
    }
}

#[tokio::test]
async fn detects_sensitive_additions_removals_list_order_and_backend_defaults() {
    for (local_fields, stored_fields) in [
        (json!({"data":{"private-added":"private-value"}}), json!({})),
        (json!({}), json!({"data":{"private-removed":"private-value"}})),
        (
            json!({"items":["private-a","private-b"]}),
            json!({"items":["private-b","private-a"]}),
        ),
        (json!({}), json!({"type":"Opaque"})),
    ] {
        let server = MockServer::start().await;
        let mut local = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"sample","namespace":"default"}});
        let mut stored = local.clone();
        local
            .as_object_mut()
            .unwrap()
            .extend(local_fields.as_object().unwrap().clone());
        stored
            .as_object_mut()
            .unwrap()
            .extend(stored_fields.as_object().unwrap().clone());
        source(&server, SECRET_PATH, ResponseTemplate::new(200).set_body_json(stored)).await;
        let output = run(&server.uri(), &[&local.to_string()], &["--token", "diff-token"]).await;
        assert_report(output, 1, "document 1: changed (content)\n");
        assert_reads(&server, 1).await;
    }
}

#[tokio::test]
async fn compares_source_documents_that_omit_the_kind_selected_by_the_get_route() {
    let server = MockServer::start().await;
    source(
        &server,
        SECRET_PATH,
        ResponseTemplate::new(200)
            .set_body_json(json!({"apiVersion":"v1","metadata":{"name":"sample","namespace":"default"}})),
    )
    .await;
    assert_report(
        run(&server.uri(), &[SECRET], &["--token", "diff-token"]).await,
        1,
        "document 1: changed (content)\n",
    );
    assert_reads(&server, 1).await;
}

#[tokio::test]
async fn tokenless_equal_comparison_uses_only_the_auth_probe_and_source_get() {
    let server = MockServer::start().await;
    for (endpoint, body) in [
        (
            "/api/v1/auth/status",
            json!({"success":true,"data":{"auth_required":false,"mode":"token","status":"ready"}}),
        ),
        (
            SECRET_PATH,
            json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"sample","namespace":"default"}}),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    assert_report(
        run(&format!("{}/", server.uri()), &[SECRET], &[]).await,
        0,
        "document 1: equal\n",
    );
    assert_reads(&server, 2).await;
}

#[tokio::test]
async fn tokenless_controller_states_report_the_required_action_without_reading_resources() {
    for (status, expected) in [
        (
            ResponseTemplate::new(200).set_body_json(
                json!({"success":true,"data":{"auth_required":true,"mode":"private-mode","status":"ready"}}),
            ),
            "--token",
        ),
        (
            ResponseTemplate::new(200).set_body_json(
                json!({"success":true,"data":{"auth_required":true,"mode":"token","status":"initializing"}}),
            ),
            "retry later",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"private-response":"invalid"})),
            "invalid Controller authentication response",
        ),
        (ResponseTemplate::new(503).set_body_string("private-reason"), "HTTP 503"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/status"))
            .respond_with(status)
            .expect(1)
            .mount(&server)
            .await;
        assert!(assert_failure(run(&server.uri(), &[SECRET], &[]).await).contains(expected));
        assert_reads(&server, 1).await;
    }
}

#[tokio::test]
async fn comparison_failures_suppress_partial_results_and_keep_diagnostics_distinct() {
    let mut messages = Vec::new();
    for response in [
        ResponseTemplate::new(403).set_body_string("private-denial"),
        ResponseTemplate::new(200).set_body_string("private-invalid-json"),
        ResponseTemplate::new(200)
            .set_body_json(json!({"kind":"private-wrong-kind","metadata":{"name":"sample","namespace":"default"}})),
        ResponseTemplate::new(200).set_body_json(
            json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"private-other","namespace":"default"}}),
        ),
        ResponseTemplate::new(404).set_body_string("private-proxy-error"),
        ResponseTemplate::new(404),
        ResponseTemplate::new(404)
            .insert_header("x-edgion-source-not-found", "true")
            .set_body_string("private-proxy-error"),
    ] {
        let server = MockServer::start().await;
        source(
            &server,
            "/api/v1/namespaced/Secret/default/absent",
            ResponseTemplate::new(404).insert_header("x-edgion-source-not-found", "true"),
        )
        .await;
        source(&server, SECRET_PATH, response).await;
        let output = run(
            &server.uri(),
            &[&SECRET.replace("sample", "absent"), SECRET],
            &["--token", "diff-token"],
        )
        .await;
        messages.push(assert_failure(output));
        assert_reads(&server, 2).await;
    }
    assert_ne!(messages[0], messages[1]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/private-url", listener.local_addr().unwrap());
    drop(listener);
    let network = assert_failure(run(&url, &[SECRET], &["--token", "diff-token"]).await);
    assert_ne!(network, messages[0]);
    assert_ne!(network, messages[1]);
}

#[tokio::test]
async fn invalid_local_documents_and_cache_targets_fail_before_source_reads() {
    let server = MockServer::start().await;
    for manifest in [
        "apiVersion: v1\nkind: Secret\nmetadata: {name: sample}\nprivate-invalid-key: [",
        "apiVersion: v1\nkind: Secret\nmetadata: {name: '../private-path'}",
        "apiVersion: v1\nkind: Secret\nmetadata: {name: '.'}",
        "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata: {name: sample, namespace: private-namespace}",
    ] {
        assert_failure(run(&server.uri(), &[manifest], &["--token", "diff-token"]).await);
    }
    assert_failure(run(&server.uri(), &[SECRET], &["--target", "server"]).await);
    for suffix in ["?private-query", "#private-fragment"] {
        assert_failure(
            run(
                &format!("{}{suffix}", server.uri()),
                &[SECRET],
                &["--token", "diff-token"],
            )
            .await,
        );
    }
    for userinfo in ["private-user", ":private-password", "private-user:private-password"] {
        let url = server.uri().replacen("://", &format!("://{userinfo}@"), 1);
        for flags in [&[][..], &["--token", "diff-token"][..]] {
            assert_failure(run(&url, &[SECRET], flags).await);
        }
    }
    assert_reads(&server, 0).await;
}
