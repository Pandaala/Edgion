// LDAP Auth TLS / CA / mTLS integration test suite.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose, Engine as _};
use std::time::Instant;

pub struct LdapAuthTlsTestSuite;

const TLS_CA_HOST: &str = "ldap-auth-tls-ca.example.com";
const TLS_MTLS_HOST: &str = "ldap-auth-tls-mtls.example.com";
const TLS_WRONG_CA_HOST: &str = "ldap-auth-tls-wrong-ca.example.com";
const TLS_STARTTLS_HOST: &str = "ldap-auth-tls-starttls.example.com";

impl LdapAuthTlsTestSuite {
    fn encode_header(scheme: &str, username: &str, password: &str) -> String {
        let raw = format!("{}:{}", username, password);
        let encoded = general_purpose::STANDARD.encode(raw);
        format!("{} {}", scheme, encoded)
    }

    /// Scenario 1: LDAPS with correct private CA via SecretRef → bind succeeds, 200 returned.
    fn test_private_ca_secretref_bind_succeeds() -> TestCase {
        TestCase::new(
            "private_ca_secretref_bind_succeeds",
            "LDAPS + private CA via SecretRef: valid user gets 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TLS_CA_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Correct private CA: LDAP bind succeeded".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 with valid CA, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Scenario 2: LDAPS with wrong CA (inline PEM) → TLS handshake fails → plugin fail-closed 503.
    fn test_wrong_ca_returns_503() -> TestCase {
        TestCase::new(
            "wrong_ca_returns_503",
            "LDAPS + mismatched CA: TLS handshake fails → 503",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TLS_WRONG_CA_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 503 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Wrong CA: got expected 503 (fail-closed)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 503 with mismatched CA, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Scenario 3: LDAPS with server requiring mTLS + clientSecretRef supplied → bind succeeds, 200.
    fn test_mtls_client_cert_bind_succeeds() -> TestCase {
        TestCase::new(
            "mtls_client_cert_bind_succeeds",
            "LDAPS + mTLS: server requires client cert, clientSecretRef supplied → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TLS_MTLS_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "mTLS: client cert accepted, bind succeeded".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 with valid client cert, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Scenario 4: STARTTLS + custom CA injected via set_config → TLS upgrade path, 200.
    fn test_starttls_with_custom_ca() -> TestCase {
        TestCase::new(
            "starttls_custom_ca_bind_succeeds",
            "STARTTLS + private CA: TLS config injected on STARTTLS upgrade path → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TLS_STARTTLS_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "STARTTLS + private CA: upgrade succeeded, bind OK".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 on STARTTLS path, got {}", status),
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

impl TestSuite for LdapAuthTlsTestSuite {
    fn name(&self) -> &str {
        "LdapAuthTls"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_private_ca_secretref_bind_succeeds(),
            Self::test_wrong_ca_returns_503(),
            Self::test_mtls_client_cert_bind_succeeds(),
            Self::test_starttls_with_custom_ca(),
        ]
    }
}
