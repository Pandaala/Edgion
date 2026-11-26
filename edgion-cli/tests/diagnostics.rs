use serde_json::{json, Value};
use std::process::{Command, Output};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cli(url: &str, report: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_edgion-cli"));
    command
        .args(["--server", url, "--token", "cli-test", "diagnostics", report])
        .env_remove("EDGION_CA_CERT")
        .env("NO_PROXY", "*");
    command
}

async fn run_report(command: &'static str, endpoint: &str, response: ResponseTemplate) -> Output {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(header("authorization", "Bearer cli-test"))
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;
    let url = server.uri();
    tokio::task::spawn_blocking(move || cli(&url, command).output().unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn interrupted_report_redacts_request_url() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/private-review-path", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = BufReader::new(&mut stream).lines();
        while let Some(line) = headers.next_line().await.unwrap() {
            if line.is_empty() {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .unwrap();
    });
    let output = tokio::task::spawn_blocking(move || cli(&url, "conflicts").output().unwrap())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("Failed to read Controller diagnostics JSON"));
    assert!(!stderr.contains("private-review-path"));
}

#[tokio::test]
async fn prints_controller_reports_including_empty_results_and_truncation() {
    for (command, endpoint, data) in [
        ("conflicts", "conf-conflicts", json!({"conflicts": []})),
        (
            "conflicts",
            "conf-conflicts",
            json!({"conflicts": [{
            "kind": "Service", "key": "default/api", "winner": "a.yaml", "losers": ["b.yaml"], "extra": true
        }], "extra": 1}),
        ),
        (
            "unknown-fields",
            "unknown-fields",
            json!({"total": 2, "truncated": true, "items": [
                {"kind": "LinkSys", "namespace": "default", "name": "cache", "paths": ["foo"]}
            ]}),
        ),
    ] {
        let report = json!({"success": true, "data": data});
        let output = run_report(
            command,
            &format!("/api/v1/diagnostics/{endpoint}"),
            ResponseTemplate::new(200).set_body_json(&report),
        )
        .await;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), report);
    }
}

#[tokio::test]
async fn rejects_wrong_report_and_malformed_required_fields() {
    for (command, endpoint, data) in [
        (
            "conflicts",
            "conf-conflicts",
            json!({"total": 0, "truncated": false, "items": []}),
        ),
        ("unknown-fields", "unknown-fields", json!({"conflicts": []})),
        (
            "unknown-fields",
            "unknown-fields",
            json!({"total": "0", "truncated": false, "items": []}),
        ),
        (
            "unknown-fields",
            "unknown-fields",
            json!({"total": 1, "truncated": false, "items": [{}]}),
        ),
        (
            "conflicts",
            "conf-conflicts",
            json!({"conflicts": [{"kind": "Service", "key": "api", "winner": "a", "losers": [1]}]}),
        ),
    ] {
        let output = run_report(
            command,
            &format!("/api/v1/diagnostics/{endpoint}"),
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": data})),
        )
        .await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test]
async fn unsuccessful_queries_exit_without_printing_a_report() {
    for status in [403, 503, 200] {
        let output = run_report(
            "conflicts",
            "/api/v1/diagnostics/conf-conflicts",
            ResponseTemplate::new(status).set_body_json(json!({"success": false})),
        )
        .await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        if status != 200 {
            assert!(String::from_utf8_lossy(&output.stderr).contains(&status.to_string()));
        }
    }
}

#[tokio::test]
async fn standby_hints_only_display_the_leader_origin() {
    for (body, expected) in [
        (
            json!({"error":"not leader","leader":"leader.example:12101"}),
            "at leader.example:12101",
        ),
        (
            json!({"error":"not leader","leader":"[2001:db8::1]:80"}),
            "at [2001:db8::1]:80",
        ),
        (
            json!({"error":"not leader","leader":"/private-marker:80"}),
            "retry on the active Controller",
        ),
        (
            json!({"error":"not leader","leader":"https://private-marker@leader.example/private-marker?q=private-marker"}),
            "https://leader.example",
        ),
        (
            json!({"error":"not leader","leader":null}),
            "retry on the active Controller",
        ),
        (
            json!({"error":"private-marker","leader":"leader.example:12101"}),
            "HTTP 409 Conflict",
        ),
    ] {
        let output = run_report(
            "conflicts",
            "/api/v1/diagnostics/conf-conflicts",
            ResponseTemplate::new(409).set_body_json(body),
        )
        .await;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(stderr.contains(expected));
        assert!(!stderr.contains("private-marker"));
    }
}

#[tokio::test]
async fn downstream_closing_stdout_finishes_quietly() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/diagnostics/conf-conflicts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": {"conflicts": []}})))
        .expect(1)
        .mount(&server)
        .await;
    let url = server.uri();
    let output = tokio::task::spawn_blocking(move || {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        cli(&url, "conflicts").stdout(writer).output().unwrap()
    })
    .await
    .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
}
