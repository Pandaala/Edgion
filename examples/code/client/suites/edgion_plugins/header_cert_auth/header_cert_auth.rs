// Header Cert Auth Plugin Test Suite

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_yaml::Value;
use std::time::Instant;

pub struct HeaderCertAuthTestSuite;

const TEST_HOST: &str = "header-cert-auth-test.example.com";
const CERT_HEADER_NAME: &str = "X-Client-Cert";

const COMPILE_TIME_SECRET_YAML: &str =
    include_str!("../../../../../test/conf/HTTPRoute/Backend/BackendTLS/ClientCert_edge_backend-client-cert.yaml");

const RUNTIME_SECRET_RELATIVE: &str =
    "generated-secrets/HTTPRoute/Backend/BackendTLS/ClientCert_edge_backend-client-cert.yaml";
const K8S_MOUNTED_CERT_PATH: &str = "/usr/local/edgion/examples/test/certs/mtls/valid-client.crt";
const REPO_CERT_PATH: &str = "examples/test/certs/mtls/valid-client.crt";

fn read_secret_data_field(yaml: &str, key: &str) -> Option<String> {
    let value: Value = serde_yaml::from_str(yaml).ok()?;
    let data = value.get("data")?.as_mapping()?;
    data.get(Value::String(key.to_string()))?
        .as_str()
        .map(|s| s.to_string())
}

fn load_runtime_secret_yaml() -> Option<String> {
    let work_dir = std::env::var("EDGION_WORK_DIR").ok()?;
    let path = std::path::Path::new(&work_dir).join(RUNTIME_SECRET_RELATIVE);
    std::fs::read_to_string(&path).ok()
}

fn load_client_cert_from_known_paths() -> Option<String> {
    [K8S_MOUNTED_CERT_PATH, REPO_CERT_PATH]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
}

fn load_client_cert_pem() -> Result<String, String> {
    if let Some(yaml) = load_runtime_secret_yaml() {
        let cert_b64 = read_secret_data_field(&yaml, "tls.crt")
            .ok_or_else(|| "missing tls.crt in runtime-generated test fixture".to_string())?;
        let cert_bytes = STANDARD
            .decode(cert_b64.as_bytes())
            .map_err(|e| format!("failed to decode tls.crt base64: {}", e))?;
        return String::from_utf8(cert_bytes).map_err(|e| format!("tls.crt is not utf8 pem: {}", e));
    }

    if let Some(cert) = load_client_cert_from_known_paths() {
        return Ok(cert);
    }

    let cert_b64 = read_secret_data_field(COMPILE_TIME_SECRET_YAML, "tls.crt")
        .ok_or_else(|| "missing tls.crt in runtime, mounted, and compile-time fixtures".to_string())?;
    let cert_bytes = STANDARD
        .decode(cert_b64.as_bytes())
        .map_err(|e| format!("failed to decode tls.crt base64: {}", e))?;
    String::from_utf8(cert_bytes).map_err(|e| format!("tls.crt is not utf8 pem: {}", e))
}

fn build_url_encoded_cert_header() -> Result<String, String> {
    let pem = load_client_cert_pem()?;
    Ok(utf8_percent_encode(&pem, NON_ALPHANUMERIC).to_string())
}

impl HeaderCertAuthTestSuite {
    fn test_valid_header_cert_returns_200() -> TestCase {
        TestCase::new(
            "valid_header_cert_returns_200",
            "Valid X-Client-Cert header returns 200 and injects cert identity headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let cert_header = match build_url_encoded_cert_header() {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header(CERT_HEADER_NAME, cert_header);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            let has_identity =
                                body.contains("x-consumer-username") && body.contains("client1.example.com");
                            let has_fingerprint = body.contains("x-client-cert-fingerprint");

                            if has_identity && has_fingerprint {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Header cert verified and identity headers injected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Missing injected headers. identity={}, fingerprint={}, body={}",
                                        has_identity, has_fingerprint, body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_missing_cert_header_returns_401() -> TestCase {
        TestCase::new(
            "missing_cert_header_returns_401",
            "Missing certificate header returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx.http_client.get(&url).header("host", TEST_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Missing certificate header rejected with 401".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_hide_credentials_removes_source_header() -> TestCase {
        TestCase::new(
            "hide_credentials_removes_source_header",
            "hideCredentials removes source X-Client-Cert header before upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let cert_header = match build_url_encoded_cert_header() {
                        Ok(v) => v,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header(CERT_HEADER_NAME, cert_header);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            let source_removed = !body.contains("x-client-cert:");
                            let fingerprint_kept = body.contains("x-client-cert-fingerprint");

                            if source_removed && fingerprint_kept {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Source cert header removed and derived fingerprint header kept".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Header stripping mismatch. source_removed={}, fingerprint_kept={}, body={}",
                                        source_removed, fingerprint_kept, body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for HeaderCertAuthTestSuite {
    fn name(&self) -> &str {
        "HeaderCertAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_valid_header_cert_returns_200(),
            Self::test_missing_cert_header_returns_401(),
            Self::test_hide_credentials_removes_source_header(),
        ]
    }
}
