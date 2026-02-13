// LDAP Auth Plugin Test Suite

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose, Engine as _};
use std::time::Instant;

pub struct LdapAuthTestSuite;

const BASE_HOST: &str = "ldap-auth-test.example.com";
const ANON_HOST: &str = "ldap-auth-anonymous.example.com";
const BASIC_SCHEME_HOST: &str = "ldap-auth-basic-scheme.example.com";
const HIDE_CREDS_HOST: &str = "ldap-auth-hide-creds.example.com";

impl LdapAuthTestSuite {
    fn encode_header(scheme: &str, username: &str, password: &str) -> String {
        let raw = format!("{}:{}", username, password);
        let encoded = general_purpose::STANDARD.encode(raw);
        format!("{} {}", scheme, encoded)
    }

    /// No credential and no anonymous should return 401.
    fn test_missing_credentials_returns_401() -> TestCase {
        TestCase::new(
            "missing_credentials_returns_401",
            "Missing Authorization returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    match ctx.http_client.get(&url).header("host", BASE_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 401 {
                                return TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status));
                            }

                            let www_auth = resp
                                .headers()
                                .get("WWW-Authenticate")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            if www_auth.to_lowercase().starts_with("ldap ") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 401 with WWW-Authenticate: {}", www_auth),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Unexpected WWW-Authenticate header: {}", www_auth),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Invalid credential format should return 401.
    fn test_invalid_authorization_returns_401() -> TestCase {
        TestCase::new(
            "invalid_authorization_returns_401",
            "Invalid Authorization format returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASE_HOST)
                        .header("Authorization", "ldap not-base64!!!")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid credential format rejected with 401".to_string(),
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

    /// Valid LDAP credentials should return 200 and set credential header.
    fn test_valid_credentials_returns_200() -> TestCase {
        TestCase::new(
            "valid_credentials_returns_200",
            "Valid LDAP credentials return 200 and inject X-Credential-Identifier",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASE_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = resp.text().await.unwrap_or_default().to_lowercase();
                            if body.contains("x-credential-identifier") && body.contains("alice") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid LDAP credentials authenticated and header injected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Missing credential header in upstream request body: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// hideCredentials=true should remove Authorization before forwarding upstream.
    fn test_hide_credentials_removes_authorization() -> TestCase {
        TestCase::new(
            "hide_credentials_removes_authorization",
            "hideCredentials=true removes Authorization from upstream request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HIDE_CREDS_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = resp.text().await.unwrap_or_default().to_lowercase();
                            if body.contains("authorization") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Authorization was not removed from upstream headers: {}", body),
                                );
                            }

                            if body.contains("x-credential-identifier") && body.contains("alice") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Authorization removed and credential header preserved".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Missing credential header after auth: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Invalid password should return 401.
    fn test_invalid_password_returns_401() -> TestCase {
        TestCase::new(
            "invalid_password_returns_401",
            "Wrong LDAP password returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("ldap", "alice", "wrong-password");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASE_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Wrong LDAP password rejected with 401".to_string(),
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

    /// Anonymous mode should pass and inject marker headers.
    fn test_anonymous_access_sets_headers() -> TestCase {
        TestCase::new(
            "anonymous_access_sets_headers",
            "Anonymous mode allows no-credential request and sets marker headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());

                    match ctx.http_client.get(&url).header("host", ANON_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = resp.text().await.unwrap_or_default().to_lowercase();
                            let has_user = body.contains("x-credential-identifier") && body.contains("guest-user");
                            let has_anon = body.contains("x-anonymous-consumer") && body.contains("true");

                            if has_user && has_anon {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Anonymous headers injected correctly".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Missing anonymous headers in upstream request body: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Proxy-Authorization has higher priority than Authorization.
    fn test_proxy_authorization_priority() -> TestCase {
        TestCase::new(
            "proxy_authorization_priority",
            "Invalid Proxy-Authorization should win over valid Authorization and return 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let valid_auth = Self::encode_header("ldap", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASE_HOST)
                        .header("Proxy-Authorization", "ldap !!!invalid-base64!!!")
                        .header("Authorization", valid_auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Proxy-Authorization priority validated".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 401 due to invalid proxy auth, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// headerType=basic should accept Basic scheme and authenticate.
    fn test_basic_scheme_header_type() -> TestCase {
        TestCase::new(
            "basic_scheme_header_type",
            "headerType=basic accepts Basic auth header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = Self::encode_header("Basic", "alice", "password123");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BASIC_SCHEME_HOST)
                        .header("Authorization", auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Basic scheme parsed and authenticated successfully".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for LdapAuthTestSuite {
    fn name(&self) -> &str {
        "LdapAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_missing_credentials_returns_401(),
            Self::test_invalid_authorization_returns_401(),
            Self::test_valid_credentials_returns_200(),
            Self::test_invalid_password_returns_401(),
            Self::test_anonymous_access_sets_headers(),
            Self::test_proxy_authorization_priority(),
            Self::test_basic_scheme_header_type(),
            Self::test_hide_credentials_removes_authorization(),
        ]
    }
}
