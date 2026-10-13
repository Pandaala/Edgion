use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "condition-refs-dynamic.example.com";
const INITIAL_TOKEN: &str = "initial-token";
const ROTATED_TOKEN: &str = "rotated-token";

async fn request(ctx: &TestContext, token: &str) -> Result<(u16, String, Option<String>, Option<String>), String> {
    let response = ctx
        .http_client
        .post(format!("http://{}:31180/echo", ctx.target_host))
        .header("host", HOST)
        .header("X-Body-Token", token)
        .header("X-Dye-Token", token)
        .body("abcdef")
        .send()
        .await
        .map_err(|error| format!("Condition reference request failed: {error}"))?;
    let status = response.status().as_u16();
    let dye = response
        .headers()
        .get("X-Condition-Ref-Dye")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body_marker = response
        .headers()
        .get("X-Condition-Ref-Body")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .text()
        .await
        .map_err(|error| format!("Condition reference response body failed: {error}"))?;
    Ok((status, body, dye, body_marker))
}

async fn assert_active_token(ctx: &TestContext, active: &str, stale: &str) -> TestResult {
    let start = Instant::now();
    let active_result = request(ctx, active).await;
    let stale_result = request(ctx, stale).await;
    match (active_result, stale_result) {
        (Ok((200, _, active_dye, active_body)), Ok((500, stale_body, stale_dye, stale_marker)))
            if active_dye.as_deref() == Some("matched")
                && active_body.as_deref() == Some("matched")
                && stale_body == "Request body processing failed"
                && stale_dye.is_none()
                && stale_marker.is_none() =>
        {
            TestResult::passed(start.elapsed())
        }
        (active_result, stale_result) => TestResult::failed(
            start.elapsed(),
            format!(
                "Unexpected active/stale condition reference results: active={active_result:?}, stale={stale_result:?}"
            ),
        ),
    }
}

pub struct ConditionRefsDynamicInitialTestSuite;

impl TestSuite for ConditionRefsDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Initial"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_initial",
            "Initial Secret value enables body and Dye conditions",
            |ctx| Box::pin(async move { assert_active_token(&ctx, INITIAL_TOKEN, ROTATED_TOKEN).await }),
        )]
    }
}

pub struct ConditionRefsDynamicUpdatedTestSuite;

impl TestSuite for ConditionRefsDynamicUpdatedTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Updated"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_updated",
            "Rotated Secret value replaces stale body and Dye matches",
            |ctx| Box::pin(async move { assert_active_token(&ctx, ROTATED_TOKEN, INITIAL_TOKEN).await }),
        )]
    }
}

pub struct ConditionRefsDynamicDeletedTestSuite;

impl TestSuite for ConditionRefsDynamicDeletedTestSuite {
    fn name(&self) -> &str {
        "Condition References Dynamic - Deleted"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_dynamic_deleted",
            "Deleting the shared condition Secret fails the plugin resource closed",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match request(&ctx, INITIAL_TOKEN).await {
                        Ok((500, body, dye, body_marker))
                            if body.contains("plugin configuration error")
                                && dye.is_none()
                                && body_marker.is_none() =>
                        {
                            TestResult::passed(start.elapsed())
                        }
                        result => TestResult::failed(
                            start.elapsed(),
                            format!("Expected generic 500 without Dye after Secret deletion, got {result:?}"),
                        ),
                    }
                })
            },
        )]
    }
}

async fn assert_live_ip_statuses(ctx: &TestContext, ordinary: u16, rap: u16) -> TestResult {
    let start = Instant::now();
    for (host, expected) in [
        ("condition-refs-live-gate.example.com", ordinary),
        ("condition-refs-live-rap.example.com", rap),
    ] {
        let response = ctx
            .http_client
            .get(format!("http://{}:31180/echo", ctx.target_host))
            .header("host", host)
            .send()
            .await;
        match response {
            Ok(response) if response.status().as_u16() == expected => {}
            result => {
                return TestResult::failed(
                    start.elapsed(),
                    format!("Live IpList consumer {host} expected {expected}, got {result:?}"),
                )
            }
        }
    }
    TestResult::passed(start.elapsed())
}

macro_rules! live_ip_suite {
    ($name:ident, $title:literal, $case:literal, $ordinary:literal, $rap:literal) => {
        pub struct $name;
        impl TestSuite for $name {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![TestCase::new(
                    $case,
                    "IpList-only publication changes retained ordinary and RAP consumers",
                    |ctx| Box::pin(async move { assert_live_ip_statuses(&ctx, $ordinary, $rap).await }),
                )]
            }
        }
    };
}
live_ip_suite!(
    ConditionRefsLiveIpInitialTestSuite,
    "Condition References Dynamic - IpList Initial",
    "condition_refs_live_ip_initial",
    418,
    200
);
live_ip_suite!(
    ConditionRefsLiveIpUpdatedTestSuite,
    "Condition References Dynamic - IpList Updated",
    "condition_refs_live_ip_updated",
    200,
    403
);
live_ip_suite!(
    ConditionRefsLiveIpDeletedTestSuite,
    "Condition References Dynamic - IpList Deleted",
    "condition_refs_live_ip_deleted",
    500,
    403
);

async fn real_ip_echo(ctx: &TestContext, host: &str, forwarded: bool) -> Result<String, String> {
    let mut request = ctx
        .http_client
        .get(format!("http://{}:31180/headers", ctx.target_host))
        .header("host", host);
    if forwarded {
        request = request.header("x-forwarded-for", "203.0.113.77");
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.status().as_u16() != 200 {
        return Err(format!("expected 200, got {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    body["headers"]["x-real-ip"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "missing x-real-ip echo".into())
}

async fn assert_live_real_ip(ctx: &TestContext, plugin_trusted: bool, global_trusted: bool) -> TestResult {
    let start = Instant::now();
    for (host, trusted) in [
        ("condition-refs-live-realip.example.com", plugin_trusted),
        ("condition-refs-global-realip.example.com", global_trusted),
    ] {
        let result = async {
            let peer = real_ip_echo(ctx, host, false).await?;
            let forwarded = real_ip_echo(ctx, host, true).await?;
            let expected = if trusted { "203.0.113.77" } else { &peer };
            if forwarded == expected {
                Ok(())
            } else {
                Err(format!("{host}: expected {expected}, got {forwarded}"))
            }
        }
        .await;
        if let Err(error) = result {
            return TestResult::failed(start.elapsed(), error);
        }
    }
    TestResult::passed(start.elapsed())
}

macro_rules! real_ip_suite {
    ($name:ident, $title:literal, $case:literal, $plugin:literal, $global:literal) => {
        pub struct $name;
        impl TestSuite for $name {
            fn name(&self) -> &str {
                $title
            }
            fn test_cases(&self) -> Vec<TestCase> {
                vec![TestCase::new(
                    $case,
                    "RealIp references respond to content, availability and visibility without rebuilding",
                    |ctx| Box::pin(async move { assert_live_real_ip(&ctx, $plugin, $global).await }),
                )]
            }
        }
    };
}
real_ip_suite!(
    ConditionRefsRealIpTrustedTestSuite,
    "Condition References Dynamic - RealIp Trusted",
    "condition_refs_realip_trusted",
    true,
    true
);
real_ip_suite!(
    ConditionRefsRealIpPluginFallbackTestSuite,
    "Condition References Dynamic - RealIp Plugin Fallback",
    "condition_refs_realip_plugin_fallback",
    false,
    true
);
real_ip_suite!(
    ConditionRefsRealIpGlobalFallbackTestSuite,
    "Condition References Dynamic - RealIp Global Fallback",
    "condition_refs_realip_global_fallback",
    true,
    false
);
