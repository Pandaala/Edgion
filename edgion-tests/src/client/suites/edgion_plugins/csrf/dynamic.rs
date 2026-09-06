//! CSRF Secret-rotation phase suites.
//!
//! Token and signing-key values stay out of failure messages. Suites only
//! report HTTP status and whether a CSRF cookie was present.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::sync::Mutex;
use std::time::Instant;

const HOST: &str = "csrf-secret-rotation.example.com";
const TOKEN_HEADER: &str = "X-CSRF-Token";

static PRE_ROTATION_TOKEN: Mutex<Option<String>> = Mutex::new(None);

fn token_slot() -> std::sync::MutexGuard<'static, Option<String>> {
    PRE_ROTATION_TOKEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn store_csrf_pre_rotation_token(token: String) {
    *token_slot() = Some(token);
}

fn clear_csrf_pre_rotation_token() {
    *token_slot() = None;
}

pub struct CsrfPreRotationTokenGuard;

impl Drop for CsrfPreRotationTokenGuard {
    fn drop(&mut self) {
        clear_csrf_pre_rotation_token();
    }
}

pub fn capture_csrf_pre_rotation_token(token: String) -> CsrfPreRotationTokenGuard {
    store_csrf_pre_rotation_token(token);
    CsrfPreRotationTokenGuard
}

fn pre_rotation_token() -> Option<String> {
    token_slot().clone()
}

fn extract_csrf_token(set_cookie: &str, name: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn cookie_has_token_name(response: &reqwest::Response, name: &str) -> bool {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|cookie| extract_csrf_token(cookie, name).is_some())
}

fn issued_token(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| extract_csrf_token(cookie, name))
}

async fn issue_and_validate(ctx: &TestContext) -> TestResult {
    let start = Instant::now();
    let get_url = format!("{}/health", ctx.http_url());
    let post_url = format!("{}/echo", ctx.http_url());

    let get_resp = match ctx.http_client.get(&get_url).header("host", HOST).send().await {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("CSRF issuance request failed: {error}")),
    };

    let get_status = get_resp.status().as_u16();
    if get_status != 200 {
        return TestResult::failed(
            start.elapsed(),
            format!("expected CSRF issuance status 200, got {get_status}"),
        );
    }

    let Some(token) = issued_token(&get_resp, TOKEN_HEADER) else {
        return TestResult::failed(
            start.elapsed(),
            "CSRF issuance response did not include a token cookie".into(),
        );
    };

    let post_resp = match ctx
        .http_client
        .post(&post_url)
        .header("host", HOST)
        .header("cookie", format!("{TOKEN_HEADER}={token}"))
        .header(TOKEN_HEADER, &token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("CSRF validation request failed: {error}")),
    };

    let post_status = post_resp.status().as_u16();
    if post_status == 200 {
        TestResult::passed(start.elapsed())
    } else {
        TestResult::failed(
            start.elapsed(),
            format!("expected CSRF validation status 200, got {post_status}"),
        )
    }
}

async fn post_status(ctx: &TestContext, token: &str) -> Result<u16, String> {
    let response = ctx
        .http_client
        .post(format!("{}/echo", ctx.http_url()))
        .header("host", HOST)
        .header("cookie", format!("{TOKEN_HEADER}={token}"))
        .header(TOKEN_HEADER, token)
        .send()
        .await
        .map_err(|error| format!("CSRF pre-rotation token probe failed: {error}"))?;
    Ok(response.status().as_u16())
}

async fn assert_rotated_generation(ctx: &TestContext) -> TestResult {
    let start = Instant::now();
    let Some(stale_token) = pre_rotation_token() else {
        return TestResult::failed(
            start.elapsed(),
            "rotated CSRF phase is missing the captured pre-rotation token".into(),
        );
    };

    match post_status(ctx, &stale_token).await {
        Ok(401) => {}
        Ok(status) => {
            return TestResult::failed(
                start.elapsed(),
                format!("expected 401 for pre-rotation CSRF token, got {status}"),
            );
        }
        Err(error) => return TestResult::failed(start.elapsed(), error),
    }

    issue_and_validate(ctx).await
}

async fn assert_fail_closed(ctx: &TestContext) -> TestResult {
    let start = Instant::now();
    let url = format!("{}/health", ctx.http_url());
    let response = match ctx.http_client.get(&url).header("host", HOST).send().await {
        Ok(response) => response,
        Err(error) => return TestResult::failed(start.elapsed(), format!("CSRF fail-closed request failed: {error}")),
    };

    let status = response.status().as_u16();
    if status == 500 && !cookie_has_token_name(&response, TOKEN_HEADER) {
        TestResult::passed(start.elapsed())
    } else if status == 500 {
        TestResult::failed(
            start.elapsed(),
            "expected fail-closed 500 without a CSRF token cookie".into(),
        )
    } else {
        TestResult::failed(
            start.elapsed(),
            format!("expected CSRF fail-closed status 500, got {status}"),
        )
    }
}

pub struct CsrfSecretRotationInitialTestSuite;

impl TestSuite for CsrfSecretRotationInitialTestSuite {
    fn name(&self) -> &str {
        "Csrf Secret Rotation - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "csrf_secret_rotation_initial",
            "Current CSRF signing generation issues a token and accepts it",
            |ctx| Box::pin(async move { issue_and_validate(&ctx).await }),
        )]
    }
}

pub struct CsrfSecretRotationUpdatedTestSuite;

impl TestSuite for CsrfSecretRotationUpdatedTestSuite {
    fn name(&self) -> &str {
        "Csrf Secret Rotation - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "csrf_secret_rotation_updated",
            "Rotated CSRF signing generation rejects the pre-rotation token and accepts a new one",
            |ctx| Box::pin(async move { assert_rotated_generation(&ctx).await }),
        )]
    }
}

pub struct CsrfSecretRotationDeletedTestSuite;

impl TestSuite for CsrfSecretRotationDeletedTestSuite {
    fn name(&self) -> &str {
        "Csrf Secret Rotation - Deleted"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "csrf_secret_rotation_deleted",
            "Deleting the CSRF signing Secret fails closed with 500",
            |ctx| Box::pin(async move { assert_fail_closed(&ctx).await }),
        )]
    }
}
