// HTTP-layer IpRestriction Plugin Test Suite — allowRefs / denyRefs via central IpList
//
// All cases use ipSource: RemoteIp so the checked IP is the resolved address,
// which is the deterministic TCP peer when no forwarded address is supplied.
// The IP groups are sourced from EdgionConfigData (IpList) objects rather than
// inlined in the plugin config, proving that refs resolve from the config store.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

/// The framework awaits cases and suites serially. Capture the admitted resource
/// before mutating it, including prepared Kubernetes peer annotations.
async fn capture_original(ctx: &TestContext, kind: &str, ns: &str, name: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/api/v1/namespaced/{kind}/{ns}/{name}", ctx.admin_api_url());
    let mut request = client.get(url);
    if let Some(token) = &ctx.admin_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Cannot capture original {kind}/{ns}/{name}: {}",
            response.status()
        ));
    }
    let mut value: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    // A restoration is an unconditional publication of the original authored spec.
    value
        .as_object_mut()
        .ok_or("Original resource is not an object")?
        .remove("status");
    if let Some(metadata) = value["metadata"].as_object_mut() {
        for key in [
            "resourceVersion",
            "generation",
            "managedFields",
            "uid",
            "creationTimestamp",
            "deletionTimestamp",
            "deletionGracePeriodSeconds",
        ] {
            metadata.remove(key);
        }
    }
    Ok(value)
}

async fn apply_value(ctx: &TestContext, value: &serde_json::Value) -> Result<(), String> {
    ctx.apply_yaml(&serde_yaml::to_string(value).map_err(|e| e.to_string())?)
        .await
}

/// All restorations run even after an earlier failure. Four targets require a
/// 48s cleanup reserve. Four bounded captures take at most 20s;
/// 20s capture + 120s mutation/probes + 48s cleanup leaves 22s before 210s.
async fn restore_originals(ctx: &TestContext, originals: &[serde_json::Value]) -> Result<(), String> {
    let mut errors = Vec::new();
    for value in originals {
        match tokio::time::timeout(Duration::from_secs(12), apply_value(ctx, value)).await {
            Ok(Ok(())) => {}
            result => errors.push(format!(
                "{}/{}/{} restoration: {result:?}",
                value["kind"], value["metadata"]["namespace"], value["metadata"]["name"]
            )),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn list_with_cidr(original: &serde_json::Value, cidr: &str) -> serde_json::Value {
    let mut value = original.clone();
    value["spec"]["data"]["config"]["items"] = serde_json::json!([{"name":"live-peer", "cidrs":[cidr]}]);
    value
}

async fn wait_for_status(ctx: &TestContext, path: &str, expected: u16) -> Result<(), String> {
    let mut last = String::from("no response");
    let check = async {
        loop {
            match ctx
                .http_client
                .get(format!("{}{path}/api", ctx.http_url()))
                .header("host", "ipref.example.com")
                .send()
                .await
            {
                Ok(response) if response.status().as_u16() == expected => return,
                Ok(response) => last = format!("status {}", response.status()),
                Err(error) => last = error.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(8), check)
        .await
        .map_err(|_| format!("Expected {expected}: {last}"))
}

pub struct IpRestrictionRefsPluginTestSuite;

impl IpRestrictionRefsPluginTestSuite {
    fn live_acl_lists_and_invalid_generation_recovery() -> TestCase {
        TestCase::new("live_acl_lists_and_invalid_generation_recovery", "Live allow/deny data, omission, and typed-invalid replacement recover without rebuilding authored references", |ctx: TestContext| Box::pin(async move {
            let start = Instant::now();
            let mut originals = Vec::new();
            for (kind, name) in [("EdgionConfigData", "ipref-allowlist"), ("EdgionConfigData", "ipref-blocklist"), ("EdgionPlugins", "ip-restriction-allow-ref-hit"), ("EdgionPlugins", "ip-restriction-deny-ref-hit")] {
                match capture_original(&ctx, kind, "edgion-default", name).await {
                    Ok(value) => originals.push(value),
                    Err(error) => return TestResult::failed(start.elapsed(), error),
                }
            }
            let check = async {
                let allow = &originals[0]; let deny = &originals[1];
                apply_value(&ctx, allow).await?; apply_value(&ctx, deny).await?;
                apply_value(&ctx, &originals[2]).await?; apply_value(&ctx, &originals[3]).await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 200).await?;
                wait_for_status(&ctx, "/test/deny-ref-hit", 403).await?;
                apply_value(&ctx, &list_with_cidr(allow, "192.0.2.0/24")).await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 403).await?;
                ctx.delete_resource("EdgionConfigData", "edgion-default", "ipref-allowlist").await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 403).await?;
                apply_value(&ctx, allow).await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 200).await?;
                ctx.delete_resource("EdgionConfigData", "edgion-default", "ipref-blocklist").await?;
                wait_for_status(&ctx, "/test/deny-ref-hit", 200).await?;
                apply_value(&ctx, deny).await?;
                wait_for_status(&ctx, "/test/deny-ref-hit", 403).await?;
                let mut invalid = originals[2].clone();
                invalid["spec"]["requestPlugins"][0]["config"]["allow"] = serde_json::Value::Null;
                apply_value(&ctx, &invalid).await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 500).await?;
                apply_value(&ctx, &originals[2]).await?;
                wait_for_status(&ctx, "/test/allow-ref-hit", 200).await
            };
            let result = tokio::time::timeout(Duration::from_secs(120), check).await.unwrap_or_else(|_| Err("Live ACL phases exceeded 120s".into()));
            let cleanup = restore_originals(&ctx, &originals).await;
            match (result, cleanup) {
                (Ok(()), Ok(())) => TestResult::passed(start.elapsed()),
                (result, cleanup) => TestResult::failed(start.elapsed(), format!("check={result:?}; cleanup={cleanup:?}")),
            }
        })).with_timeout(Duration::from_secs(210))
    }

    // 1. allowRefs hit — client IP (127.0.0.1) is in the referenced allow-list → 200.
    fn test_allow_ref_hit() -> TestCase {
        TestCase::new(
            "allow_ref_hit",
            "allowRefs: client IP in referenced allow-list is permitted (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/allow-ref-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ipref.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in referenced allow list permitted with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 2. denyRefs hit — client IP (127.0.0.1) is in the referenced block-list → 403.
    fn test_deny_ref_hit() -> TestCase {
        TestCase::new(
            "deny_ref_hit",
            "denyRefs: client IP in referenced block-list is denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/deny-ref-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ipref.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in referenced block list denied with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for IpRestrictionRefsPluginTestSuite {
    fn name(&self) -> &str {
        "IpRestriction Refs Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allow_ref_hit(),
            Self::test_deny_ref_hit(),
            Self::live_acl_lists_and_invalid_generation_recovery(),
        ]
    }
}
