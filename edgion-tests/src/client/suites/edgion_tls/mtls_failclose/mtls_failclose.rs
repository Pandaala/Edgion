//! mTLS fail-close integration evidence (tls-F011, ARCH-283).
//!
//! Wire rejection is distinct from setup/transport failure. Local runs also
//! require a fresh policy audit; Kubernetes uses wire, exact publication and
//! fixture isolation because the test-client pod has no host ssl.log mount.

use super::probe::{TlsProbe, EDGE_SERVER, FRONTEND_SERVER};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use async_trait::async_trait;
use rustls::AlertDescription;
use serde_json::Value;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct MtlsFailcloseTestSuite;

const NAMESPACE: &str = "edgion-test";
const CONTROL_NAME: &str = "mtls-ca-control";
const CONTROL_HOST: &str = "mtls-ca-control.example.com";
const LISTENER: &str = "https-mtls-failclose";
const PORT: u16 = 31199;

fn listener_has_condition(
    gateway: &Value,
    listener_name: &str,
    condition_type: &str,
    expected_status: &str,
    expected_reason: &str,
) -> bool {
    gateway["status"]["listeners"]
        .as_array()
        .and_then(|listeners| listeners.iter().find(|listener| listener["name"] == listener_name))
        .and_then(|listener| listener["conditions"].as_array())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition["type"] == condition_type
                    && condition["status"] == expected_status
                    && condition["reason"] == expected_reason
            })
        })
}

fn invalid_listener_conditions(gateway: &Value, listener_name: &str) -> bool {
    listener_has_condition(
        gateway,
        listener_name,
        "ResolvedRefs",
        "False",
        "InvalidCACertificateRef",
    ) && listener_has_condition(gateway, listener_name, "Programmed", "False", "Invalid")
}

/// The listener connection gate runs before TLS, so an alert is not required.
/// TCP refusal, DNS, a stalled peer, and arbitrary request errors never pass.
pub(crate) async fn require_invalid_listener_closed(
    ctx: &TestContext,
    gateway: &str,
    listener: &str,
    hostname: &str,
    port: u16,
) -> Result<(), String> {
    let probe = TlsProbe::new(hostname, FRONTEND_SERVER, None)?;
    helpers::wait_resource_status(ctx, "Gateway", NAMESPACE, gateway, Duration::from_secs(5), |resource| {
        invalid_listener_conditions(resource, listener)
    })
    .await?;
    probe.run(&ctx.target_host, port).await?.require_peer_close()
}

fn is_k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

// The Kubernetes test-client Secret projects the exact edge-tls leaf as
// ca.pem. Select by topology, never by trying another file after a failure.
fn edge_server_fixture(k8s: bool) -> &'static str {
    if k8s {
        "edgion-tests/certs/ca.pem"
    } else {
        EDGE_SERVER
    }
}

fn ssl_log_path(ctx: &TestContext) -> PathBuf {
    ctx.access_log_path
        .parent()
        .map(|parent| parent.join("ssl.log"))
        .unwrap_or_else(|| PathBuf::from("logs/ssl.log"))
}

fn epoch_millis() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| "Cannot establish the local TLS-audit clock fence".to_string())
}

async fn advance_audit_clock(fence: i64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let now = epoch_millis()?;
        if now < fence {
            return Err("Local wall clock moved backwards before the TLS probe".to_string());
        }
        if now > fence {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Local TLS-audit clock did not advance before its deadline".to_string());
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

/// The offset excludes stored history; the connection timestamp additionally
/// excludes old records still queued in the asynchronous SSL logger.
struct AuditCursor {
    file: File,
    path: PathBuf,
    identity: Metadata,
    position: u64,
    fence: i64,
    pending: String,
}

impl AuditCursor {
    fn open(path: PathBuf, fence: i64) -> Result<Self, String> {
        let mut file =
            File::open(&path).map_err(|_| "Required local ssl.log is unreadable before the probe".to_string())?;
        let identity = file
            .metadata()
            .map_err(|_| "Cannot inspect local ssl.log".to_string())?;
        let position = identity.len();
        if position > 0 {
            file.seek(SeekFrom::End(-1))
                .map_err(|_| "Cannot inspect the ssl.log boundary".to_string())?;
            let mut tail = [0];
            file.read_exact(&mut tail)
                .map_err(|_| "Cannot read the ssl.log boundary".to_string())?;
            if tail != [b'\n'] {
                return Err("Local ssl.log has an incomplete pre-probe record".to_string());
            }
        }
        file.seek(SeekFrom::Start(position))
            .map_err(|_| "Cannot set the ssl.log evidence boundary".to_string())?;
        Ok(Self {
            file,
            path,
            identity,
            position,
            fence,
            pending: String::new(),
        })
    }

    fn observe(&mut self, hostname: &str, resource_name: &str, completed: i64) -> Result<bool, String> {
        let current = std::fs::metadata(&self.path)
            .map_err(|_| "Required local ssl.log disappeared during the probe".to_string())?;
        if !same_file(&self.identity, &current) || current.len() < self.position {
            return Err("Local ssl.log rotated or truncated across the probe".to_string());
        }
        let mut appended = String::new();
        // Read a bounded segment per poll. An oversized record cannot establish
        // evidence and must not turn this test reader into a whole-log cache.
        (&mut self.file)
            .take(64 * 1024)
            .read_to_string(&mut appended)
            .map_err(|_| "Cannot read appended TLS-audit evidence".to_string())?;
        self.position = self
            .file
            .stream_position()
            .map_err(|_| "Cannot inspect TLS-audit read position".to_string())?;
        self.pending.push_str(&appended);
        let complete_end = self.pending.rfind('\n').map(|index| index + 1).unwrap_or(0);
        let mut matched = false;
        for line in self.pending[..complete_end]
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            let entry: Value =
                serde_json::from_str(line).map_err(|_| "Appended ssl.log contains malformed JSON".to_string())?;
            matched |= audit_entry_matches(&entry, hostname, resource_name, self.fence, completed);
        }
        self.pending.drain(..complete_end);
        if self.pending.len() > 64 * 1024 {
            return Err("Appended TLS-audit record exceeds the evidence reader bound".to_string());
        }
        Ok(matched)
    }

    async fn require(mut self, hostname: &str, resource_name: &str, completed: i64) -> Result<(), String> {
        if completed <= self.fence || epoch_millis()? < completed {
            return Err("Local wall clock cannot bound this TLS-audit observation".to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if self.observe(hostname, resource_name, completed)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("No fresh target-specific mTLS config rejection audit was observed".to_string());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn audit_entry_matches(entry: &Value, hostname: &str, resource_name: &str, fence: i64, completed: i64) -> bool {
    entry["ts"]
        .as_i64()
        .is_some_and(|timestamp| timestamp > fence && timestamp <= completed)
        && entry["sni"] == hostname
        && entry["port"] == PORT
        && entry["matched"]["rsc"]["kind"] == "EdgionTls"
        && entry["matched"]["rsc"]["ns"] == NAMESPACE
        && entry["matched"]["rsc"]["name"] == resource_name
        && entry["err_log"]
            .as_str()
            .is_some_and(|error| error.starts_with("mtls config:"))
        && entry.get("handshake_complete_time").is_none()
}

async fn require_edgiontls_failclosed(ctx: &TestContext, resource: &str, hostname: &str) -> Result<(), String> {
    let server_fixture = edge_server_fixture(is_k8s_mode());
    let negative = TlsProbe::new(hostname, server_fixture, None)?;
    let control = TlsProbe::new(CONTROL_HOST, server_fixture, None)?;
    for name in [resource, CONTROL_NAME] {
        ctx.wait_for_listener_projection(
            "EdgionTls",
            NAMESPACE,
            name,
            "resolvedListenerAttachments",
            LISTENER,
            Duration::from_secs(5),
        )
        .await?;
    }
    control.run(&ctx.target_host, PORT).await?.require_complete()?;
    let audit = if is_k8s_mode() {
        None
    } else {
        // Capture the fence after opening the file. Dial only after the next
        // millisecond, so old same-millisecond queued records cannot qualify.
        let mut cursor = AuditCursor::open(ssl_log_path(ctx), 0)?;
        cursor.fence = epoch_millis()?;
        advance_audit_clock(cursor.fence).await?;
        Some(cursor)
    };
    negative
        .run(&ctx.target_host, PORT)
        .await?
        .require_alert(AlertDescription::HandshakeFailure)?;
    if let Some(cursor) = audit {
        cursor.require(hostname, resource, epoch_millis()?).await?;
    }
    Ok(())
}

impl MtlsFailcloseTestSuite {
    fn test_mtls_ca_missing_failclosed() -> TestCase {
        TestCase::new(
            "mtls_ca_missing_failclosed",
            "EdgionTls missing client CA fails closed (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_edgiontls_failclosed(&ctx, "mtls-ca-missing", "mtls-ca-missing.example.com").await {
                        Ok(()) => failclose_pass(start),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn test_mtls_ca_garbage_pem_failclosed() -> TestCase {
        TestCase::new(
            "mtls_ca_garbage_pem_failclosed",
            "EdgionTls malformed client CA fails closed (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_edgiontls_failclosed(&ctx, "mtls-ca-garbage", "mtls-ca-garbage.example.com").await {
                        Ok(()) => failclose_pass(start),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn test_frontend_mtls_garbage_ca_pem_failclosed() -> TestCase {
        TestCase::new(
            "frontend_mtls_garbage_ca_pem_failclosed",
            "Invalid frontend CA closes the listener before TLS (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_invalid_listener_closed(
                        &ctx,
                        "mtls-failclose-gateway",
                        "https-frontend-mtls-failclose",
                        "frontend-mtls-garbage.example.com",
                        31340,
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Reached the listener and observed peer closure with its exact invalid-CA conditions"
                                .to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

fn failclose_pass(start: Instant) -> TestResult {
    let evidence = if is_k8s_mode() {
        "Observed HandshakeFailure with target/control publication and same-listener serving identity control; K8s uses wire/projection/fixture isolation, not host ssl.log"
    } else {
        "Observed HandshakeFailure with target/control publication, same-listener serving identity control and fresh target-specific mTLS config audit"
    };
    TestResult::passed_with_message(start.elapsed(), evidence.to_string())
}

#[async_trait]
impl TestSuite for MtlsFailcloseTestSuite {
    fn name(&self) -> &'static str {
        "TLS mTLS Fail-Close Tests (tls-F011)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_mtls_ca_missing_failclosed(),
            Self::test_mtls_ca_garbage_pem_failclosed(),
            Self::test_frontend_mtls_garbage_ca_pem_failclosed(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn edge_serving_pin_uses_the_exact_asset_for_each_topology() {
        assert_eq!(edge_server_fixture(false), "edgion-tests/certs/server.crt");
        assert_eq!(edge_server_fixture(true), "edgion-tests/certs/ca.pem");
    }

    fn rejection(timestamp: i64) -> Value {
        json!({"ts": timestamp, "sni": "mtls-ca-missing.example.com", "port": 31199,
            "matched": {"rsc": {"kind": "EdgionTls", "ns": "edgion-test", "name": "mtls-ca-missing"}},
            "err_log": "mtls config: test rejection"})
    }

    #[test]
    fn audit_requires_current_connection_time_exact_target_and_incomplete_handshake() {
        let matches =
            |entry: &Value| audit_entry_matches(entry, "mtls-ca-missing.example.com", "mtls-ca-missing", 100, 200);
        assert!(matches(&rejection(101)));
        for timestamp in [99, 100, 201] {
            assert!(!matches(&rejection(timestamp)));
        }
        for (pointer, value) in [
            ("/sni", json!("other.example.com")),
            ("/port", json!(31340)),
            ("/matched/rsc/kind", json!("Gateway")),
            ("/matched/rsc/ns", json!("other")),
            ("/matched/rsc/name", json!("mtls-ca-garbage")),
            ("/err_log", json!("unrelated failure")),
            ("/ts", Value::Null),
        ] {
            let mut entry = rejection(101);
            *entry.pointer_mut(pointer).unwrap() = value;
            assert!(!matches(&entry));
        }
        let mut complete = rejection(101);
        complete["handshake_complete_time"] = json!(1);
        assert!(!matches(&complete));
        assert!(!matches(&json!({})));
    }

    #[test]
    fn audit_offset_and_timestamp_reject_stored_and_delayed_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssl.log");
        std::fs::write(&path, format!("{}\n", rejection(101))).unwrap();
        let mut cursor = AuditCursor::open(path.clone(), 100).unwrap();
        assert!(!cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .unwrap());
        let mut writer = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(writer, "{}", rejection(99)).unwrap();
        assert!(!cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .unwrap());
        write!(writer, "{}", rejection(102)).unwrap();
        assert!(!cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .unwrap());
        writeln!(writer).unwrap();
        assert!(cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .unwrap());
        std::fs::write(&path, "").unwrap();
        assert!(cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .is_err());
    }

    #[test]
    fn audit_rejects_rotation_and_incomplete_initial_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ssl.log");
        std::fs::write(&path, "unfinished").unwrap();
        assert!(AuditCursor::open(path.clone(), 100).is_err());
        std::fs::write(&path, "").unwrap();
        let mut cursor = AuditCursor::open(path.clone(), 100).unwrap();
        std::fs::rename(&path, dir.path().join("old.log")).unwrap();
        std::fs::write(&path, format!("{}\n", rejection(101))).unwrap();
        assert!(cursor
            .observe("mtls-ca-missing.example.com", "mtls-ca-missing", 200)
            .is_err());
    }

    #[test]
    fn invalid_conditions_belong_to_one_selected_listener() {
        let conditions = json!([
            {"type": "ResolvedRefs", "status": "False", "reason": "InvalidCACertificateRef"},
            {"type": "Programmed", "status": "False", "reason": "Invalid"}]);
        let mut gateway = json!({"status": {"listeners": [{"name": "selected", "conditions": conditions}]}});
        assert!(invalid_listener_conditions(&gateway, "selected"));
        assert!(!invalid_listener_conditions(&gateway, "other"));
        for index in [0, 1] {
            for (field, value) in [("type", "Other"), ("status", "True"), ("reason", "Other")] {
                let mut wrong = gateway.clone();
                wrong["status"]["listeners"][0]["conditions"][index][field] = json!(value);
                assert!(!invalid_listener_conditions(&wrong, "selected"));
            }
            let mut missing = gateway.clone();
            missing["status"]["listeners"][0]["conditions"]
                .as_array_mut()
                .unwrap()
                .remove(index);
            assert!(!invalid_listener_conditions(&missing, "selected"));
        }
        gateway["status"]["listeners"][0]["conditions"] = json!([]);
        assert!(!invalid_listener_conditions(&gateway, "selected"));
        assert!(!invalid_listener_conditions(&json!({}), "selected"));
    }

    fn fixture(tree: &str, name: &str) -> yaml_serde::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(tree)
            .join("conf/EdgionTls/mtls_failclose")
            .join(name);
        yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn local_and_kubernetes_fixtures_isolate_only_client_ca_from_the_control() {
        for tree in ["integration", "k8s"] {
            let control = fixture(tree, "EdgionTls_mtls-ca-control.yaml");
            assert_eq!(control["metadata"]["name"].as_str(), Some(CONTROL_NAME));
            assert_eq!(control["spec"]["hosts"][0].as_str(), Some(CONTROL_HOST));
            assert_eq!(control["spec"]["clientAuth"]["mode"].as_str(), Some("Terminate"));
            assert!(control["spec"]["clientAuth"].get("caSecretRef").is_none());
            for (name, ca) in [("missing", "nonexistent-ca"), ("garbage", "garbage-ca")] {
                let target = fixture(tree, &format!("EdgionTls_mtls-ca-{name}.yaml"));
                assert_eq!(target["metadata"]["namespace"].as_str(), Some(NAMESPACE));
                assert_eq!(
                    target["metadata"]["name"].as_str(),
                    Some(format!("mtls-ca-{name}").as_str())
                );
                assert_eq!(
                    target["spec"]["hosts"][0].as_str(),
                    Some(format!("mtls-ca-{name}.example.com").as_str())
                );
                assert_eq!(target["spec"]["clientAuth"]["mode"].as_str(), Some("Mutual"));
                assert_eq!(target["spec"]["clientAuth"]["caSecretRef"]["name"].as_str(), Some(ca));
                assert_eq!(target["spec"]["secretRef"], control["spec"]["secretRef"]);
                assert_eq!(target["spec"]["parentRefs"], control["spec"]["parentRefs"]);
                assert_eq!(target["spec"]["secretRef"]["name"].as_str(), Some("edge-tls"));
                assert_eq!(
                    target["spec"]["parentRefs"][0]["name"].as_str(),
                    Some("mtls-failclose-gateway")
                );
                assert_eq!(target["spec"]["parentRefs"][0]["sectionName"].as_str(), Some(LISTENER));
            }
            let gateway = fixture(tree, "Gateway.yaml");
            let listener = gateway["spec"]["listeners"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|listener| listener["name"].as_str() == Some(LISTENER))
                .unwrap();
            assert_eq!(listener["port"].as_u64(), Some(u64::from(PORT)));
            assert_eq!(listener["protocol"].as_str(), Some("HTTPS"));
        }
    }
}
