use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct OpenidConnectTestSuite;

const OIDC_INTROSPECTION_HOST: &str = "openid-connect-test.example.com";
const OIDC_JWKS_HOST: &str = "openid-connect-jwks-test.example.com";
const OIDC_AUTO_HOST: &str = "openid-connect-auto-test.example.com";
const OIDC_ISSUER: &str = "http://127.0.0.1:30040/oidc";
const OIDC_RS256_KID: &str = "oidc-rs256-test-key";
const OIDC_RS256_PRIVATE_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEA0UmLCqLGqy+oTAMXpajmd411/JmJ7s5ObbwVbWN7uviddI96
Yg5NtObwmXcTuDeI2cfyjRgDDLFAE7gO7BYbX3qCGw1fDPxU++Gp7FwdqrOFOcgR
jqwkzC9Ynw/C9X/qe0pkkNrP5qGFyTazcUTfTbhtNACqCmIPI/kH2vvwpbOlJ1a0
4+OUoUvG/kKvyrFAP2RX5ow38DDxDgzX0xaxhr1gIupGrrg3/y4oCe8xvQ5kM3MR
l/Xybywr/jjDigEy5jUIkGjabVo1VEBV5Q0UxZbaPZAT2/lgR5/zz9yrqnscIFos
Z03EL0piOjTP2qroJrmv2J1gENfu5bz/8HyS9QIDAQABAoIBAEm+FBPnTwE6hZ5i
6I4ieTJe0dfzcbqHTvMzdolYqFw1BaXweDrct1yqktRANN6QEtRJs5krgMeUHPPV
wsxE7dgynm1RxNAaiQdHeEwkGP/wyVyWtjkDRuP7OsqxDwzZyZEvoUe5EdA90ZoY
gBnFHrmP5kqQgVmvO459TNtIMn7vrn8nTVPy/WoQdsZlSWJrOH14UHZNAWUZDPKq
Gq0ocRhW6ILhlg3QnbsNbK6XkwMmB05L3qi+MAhyriaoP7hKprPQaAZE6CgxCClW
9TSLDBBsTIE44oH7dMu8W7EPdW7Ahm+Qng2sQDttB55RqB0pqfdum1xJaGK8mQ8e
RwBT3fECgYEA6DsCQTaE7LSX1Qnvul33fbI4+aChg50Nf5VsjXBj7nYnnSjS4uvl
4VdB7nkOuSdnpdOlWszcmxi2W9CICZAszYQINuRGK/WVYOsWxP+qszAIS5heQBgo
1vBJ8ImllbQD9rSHbRC9hhcZfsKCh7dlKScojxvhhPbuUdXZ+/3tFx8CgYEA5rVe
BmBtwyPvZtB+PcDtybpCOoCoJZnt5Zy7otxYOus/PIFGKaHYG7nY5MeBmcSwbYfm
ZzuXW/b06294hAwSwV7nSfLxbrBAGmd8rveQDJT58u90vu+EMGUz9nFqf2OemXgp
VYHRaFu2pEi4uqXkJ19u3PJtIx1npHiAotlL92sCgYBVY0UL92Deq/Rb18B2lRBn
/jzmxEI+42NQMv/r7ZRA3N7p8VXBLB2lQnEynv5j4/I/Tdex0DuZJ3f9wGoUohdn
JZHjpQGMLktTjH0dyCfapOGX8hlNldSGW2nEcMgaiEsgzfYxiwM0p4+vRRO7lRo0
DHrkS8sbGbQ9ENWKyy3+3wKBgEIp43Q6tV/Qb3jx9DJroQZIZ3P1r8NQ2NwPzfQP
8zG6g6Erhd6srpiM/PnniXB66woOfnI+sdLLCUR37H0aJUrVl8kqZjkTTN8FrMlU
8Dfbha85Iyca87MZYwSbVCqCfFqRDnGaUF74ZnHI9Ul6B+uOv/GXiNsYNMADWwjY
/qNPAoGBAJnMlja5b8S17U8hyfgrUaCYqNGEBKIBpL5NtrYbcBAtkF5+aCKJ5M+a
cTiSTzbdT00Fq8Mu62FKp3BF4q7xSX0p2fOasSd8YLHX7iwrDFTxh5rtIPTEGm8r
k7H4jvECGvmh4mhsfljw6Y+7Sl/Q1k/WqlkNPsnCvWViA+E8MKuu
-----END RSA PRIVATE KEY-----"#;

fn generate_rs256_token(sub: &str) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("clock error: {}", e))?
        .as_secs();
    let exp = now + 3600;
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(OIDC_RS256_KID.to_string());
    header.typ = Some("JWT".to_string());
    let claims = json!({
        "iss": OIDC_ISSUER,
        "sub": sub,
        "scope": "api:read profile",
        "exp": exp
    });
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(OIDC_RS256_PRIVATE_KEY.as_bytes())
            .map_err(|e| format!("invalid private key: {}", e))?,
    )
    .map_err(|e| format!("failed to sign jwt: {}", e))
}

impl OpenidConnectTestSuite {
    fn test_active_introspection_token_passes() -> TestCase {
        TestCase::new(
            "active_introspection_token_passes",
            "Active introspection token returns 200 and sets upstream headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let response = match client
                        .get(&url)
                        .header("host", OIDC_INTROSPECTION_HOST)
                        .header("Authorization", "Bearer oidc-active-token")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200, got {}", response.status().as_u16()),
                        );
                    }

                    let body = response.text().await.unwrap_or_default().to_lowercase();
                    if !body.contains("x-user-id") || !body.contains("oidc-user-123") {
                        return TestResult::failed(
                            start.elapsed(),
                            "Missing mapped claims header X-User-ID from introspection response".to_string(),
                        );
                    }
                    if !body.contains("x-access-token") || !body.contains("oidc-active-token") {
                        return TestResult::failed(
                            start.elapsed(),
                            "Missing X-Access-Token forwarded header".to_string(),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Active token accepted and upstream headers injected".to_string(),
                    )
                })
            },
        )
    }

    fn test_missing_token_returns_401() -> TestCase {
        TestCase::new(
            "missing_token_returns_401",
            "Request without token returns 401 in bearer_only mode",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let response = match client.get(&url).header("host", OIDC_INTROSPECTION_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() == 401 {
                        TestResult::passed_with_message(start.elapsed(), "Missing token rejected with 401".to_string())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401, got {}", response.status().as_u16()),
                        )
                    }
                })
            },
        )
    }

    fn test_insufficient_scope_returns_403() -> TestCase {
        TestCase::new(
            "insufficient_scope_returns_403",
            "Active token without required scope returns 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let response = match client
                        .get(&url)
                        .header("host", OIDC_INTROSPECTION_HOST)
                        .header("Authorization", "Bearer oidc-insufficient-scope-token")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() == 403 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Insufficient scope rejected with 403".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 403, got {}", response.status().as_u16()),
                        )
                    }
                })
            },
        )
    }

    fn test_inactive_token_returns_401() -> TestCase {
        TestCase::new(
            "inactive_token_returns_401",
            "Inactive introspection token returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let response = match client
                        .get(&url)
                        .header("host", OIDC_INTROSPECTION_HOST)
                        .header("Authorization", "Bearer oidc-inactive-token")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() == 401 {
                        TestResult::passed_with_message(start.elapsed(), "Inactive token rejected with 401".to_string())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401, got {}", response.status().as_u16()),
                        )
                    }
                })
            },
        )
    }

    fn test_jwks_valid_rs256_token_passes() -> TestCase {
        TestCase::new(
            "jwks_valid_rs256_token_passes",
            "Valid RS256 JWT passes with JwksOnly mode and sets headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = match generate_rs256_token("oidc-jwks-user") {
                        Ok(t) => t,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to generate JWT: {}", e));
                        }
                    };

                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", OIDC_JWKS_HOST)
                        .header("Authorization", format!("Bearer {}", jwt))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200, got {}", response.status().as_u16()),
                        );
                    }

                    let body = response.text().await.unwrap_or_default().to_lowercase();
                    if !body.contains("x-user-id") || !body.contains("oidc-jwks-user") {
                        return TestResult::failed(
                            start.elapsed(),
                            "Missing mapped claims header X-User-ID from JWKS-verified JWT".to_string(),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "RS256 JWT accepted via JWKS local verification".to_string(),
                    )
                })
            },
        )
    }

    fn test_jwks_invalid_signature_returns_401() -> TestCase {
        TestCase::new(
            "jwks_invalid_signature_returns_401",
            "Tampered RS256 JWT is rejected with 401 in JwksOnly mode",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = match generate_rs256_token("oidc-jwks-user") {
                        Ok(t) => t,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to generate JWT: {}", e));
                        }
                    };
                    let mut tampered = jwt.into_bytes();
                    if let Some(last) = tampered.last_mut() {
                        *last = if *last == b'a' { b'b' } else { b'a' };
                    }
                    let tampered = String::from_utf8_lossy(&tampered);

                    let response = match ctx
                        .http_client
                        .get(format!("{}/health", ctx.http_url()))
                        .header("host", OIDC_JWKS_HOST)
                        .header("Authorization", format!("Bearer {}", tampered))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() == 401 {
                        TestResult::passed_with_message(start.elapsed(), "Tampered JWT rejected with 401".to_string())
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401, got {}", response.status().as_u16()),
                        )
                    }
                })
            },
        )
    }

    fn test_auto_fallback_to_introspection_passes() -> TestCase {
        TestCase::new(
            "auto_fallback_to_introspection_passes",
            "Auto mode falls back to introspection when JWT format is invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let fallback_token = "oidc.auto.fallback";
                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", OIDC_AUTO_HOST)
                        .header("Authorization", format!("Bearer {}", fallback_token))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200, got {}", response.status().as_u16()),
                        );
                    }

                    let body = response.text().await.unwrap_or_default().to_lowercase();
                    if !body.contains("x-user-id") || !body.contains("oidc-user-auto") {
                        return TestResult::failed(
                            start.elapsed(),
                            "Expected auto-fallback introspection claims mapped to headers".to_string(),
                        );
                    }

                    TestResult::passed_with_message(start.elapsed(), "Auto mode fallback path validated".to_string())
                })
            },
        )
    }

    fn test_auto_mode_jwt_path_passes() -> TestCase {
        TestCase::new(
            "auto_mode_jwt_path_passes",
            "Auto mode accepts valid RS256 JWT through JWKS local verification",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = match generate_rs256_token("oidc-auto-jwt-user") {
                        Ok(t) => t,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to generate JWT: {}", e));
                        }
                    };

                    let response = match ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", OIDC_AUTO_HOST)
                        .header("Authorization", format!("Bearer {}", jwt))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    if response.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200, got {}", response.status().as_u16()),
                        );
                    }

                    let body = response.text().await.unwrap_or_default().to_lowercase();
                    if !body.contains("x-user-id") || !body.contains("oidc-auto-jwt-user") {
                        return TestResult::failed(
                            start.elapsed(),
                            "Expected JWT claims mapped in auto mode".to_string(),
                        );
                    }

                    TestResult::passed_with_message(start.elapsed(), "Auto mode JWT primary path validated".to_string())
                })
            },
        )
    }
}

impl TestSuite for OpenidConnectTestSuite {
    fn name(&self) -> &str {
        "OpenidConnect"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_active_introspection_token_passes(),
            Self::test_missing_token_returns_401(),
            Self::test_insufficient_scope_returns_403(),
            Self::test_inactive_token_returns_401(),
            Self::test_jwks_valid_rs256_token_passes(),
            Self::test_jwks_invalid_signature_returns_401(),
            Self::test_auto_fallback_to_introspection_passes(),
            Self::test_auto_mode_jwt_path_passes(),
        ]
    }
}
