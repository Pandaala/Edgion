//! Native WAF generation publication and last-known-good scenario.

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::Value;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicUpdatedTestSuite)
}

fn recovered() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicRecoveredTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDeletedTestSuite)
}

fn ordinary_body_limit() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInspectionLimitTestSuite("ordinary"))
}

fn inspection_continue() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInspectionLimitTestSuite("continue"))
}
fn inspection_reject() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInspectionLimitTestSuite("reject"))
}
fn format_prefix() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicFormatLimitTestSuite("prefix"))
}
fn format_head() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicFormatLimitTestSuite("head"))
}

fn configured_prefix() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicConfiguredPrefixTestSuite)
}

fn detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDetectionTestSuite)
}

fn injection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInjectionTestSuite)
}

fn control() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicControlTestSuite)
}

fn xml_v1() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicXmlV1TestSuite)
}
fn xml_v2() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicXmlV2TestSuite)
}
fn xml_detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicXmlDetectionTestSuite)
}

fn original_on() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicOriginalOnTestSuite)
}

fn original_detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicOriginalDetectionTestSuite)
}

fn supported_on() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSupportedOnTestSuite)
}
fn supported_detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSupportedDetectionTestSuite)
}
fn supported_standard() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSupportedStandardTestSuite)
}
fn supported_retained() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSupportedRetainedTestSuite)
}

async fn assert_supported_publication(ctx: &ScenarioCtx, profile: &str, detection: bool) -> Result<()> {
    let path = ctx.resolve("updates/to_supported_on/EdgionConfigData_waf-policy.yaml");
    let expected: Value = serde_yaml::from_str(&tokio::fs::read_to_string(path).await?)?;
    let expected_refs = &expected["spec"]["data"]["config"]["profiles"][profile]["bundleRefs"];
    for gateway in [false, true] {
        let resource = plugin_resource(ctx, gateway).await?;
        let waf = &resource["spec"]["waf"];
        ensure!(
            waf["selectedProfile"] == profile && waf["mode"] == if detection { "detectionOnly" } else { "on" },
            "supported profile/mode not published"
        );
        ensure!(
            &waf["resolvedPolicy"]["profiles"][profile]["bundleRefs"] == expected_refs,
            "supported required digest pins differ"
        );
        ensure!(
            waf["resolvedRefIndices"] == serde_json::json!([0, 1, 2]),
            "supported references not all resolved"
        );
        ensure!(
            crate::dynamic::status::conditions(&resource)?
                .iter()
                .any(|c| c["type"] == "Accepted" && c["status"] == "True"),
            "supported profile not Accepted"
        );
    }
    Ok(())
}
async fn delete_supported_resources(ctx: &ScenarioCtx) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10),async {
        let identities=tokio::fs::read_to_string(ctx.resolve("delete/supported_resources.txt")).await?;
        ensure!(identities=="HTTPRoute/edgion-default/waf-supported\nEdgionConfigData/edgion-default/crs-supported-core\nEdgionConfigData/edgion-default/crs-supported-standard\nEdgionConfigData/edgion-default/crs-supported-strict\nEdgionConfigData/edgion-default/crs-supported-wire-guard\n","supported cleanup must only target its declared resources");
        let client=crate::dynamic::apply::build_client(&ctx.controller_url)?;
        let mut errors=Vec::new();
        for identity in identities.lines(){let parts=identity.split('/').collect::<Vec<_>>();
            match client.delete(parts[0],Some(parts[1]),parts[2]).await {
                Ok(response) if response.status()==reqwest::StatusCode::NOT_FOUND=>{},
                Ok(response)=>if let Err(error)=edgion_cli::cli::client::handle_response(response).await {errors.push(error.to_string());},
                Err(error)=>errors.push(error.to_string()),
            }
        }
        ensure!(errors.is_empty(),"supported cleanup errors: {errors:?}"); Ok::<(),anyhow::Error>(())
    }).await.context("supported cleanup timed out")?
}
async fn run_supported_segment(ctx: &ScenarioCtx) -> Result<()> {
    let result = async {
        for (dir, phase, profile, detection) in [
            ("to_supported_on", "supported-on", "strict", false),
            ("to_supported_detection", "supported-detection", "strict", true),
            ("to_supported_on", "supported-restored", "strict", false),
            ("to_supported_standard", "supported-standard", "standard", false),
            ("to_supported_on", "supported-recovered", "strict", false),
        ] {
            ctx.apply_dir(&format!("updates/{dir}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(60)).await?;
            assert_supported_publication(ctx, profile, detection).await?;
        }
        run_expensive_policy_updates(ctx, SCENARIO).await?;
        let before = publication_generations(ctx).await?;
        ctx.apply_dir("updates/to_supported_invalid").await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let mut invalid = true;
            for gateway in [false, true] {
                let resource = plugin_resource(ctx, gateway).await?;
                invalid &= crate::dynamic::status::conditions(&resource)?
                    .iter()
                    .any(|c| c["type"] == "Accepted" && c["status"] == "False");
                invalid &= resource["spec"]["waf"]["resolutionErrors"]
                    .as_array()
                    .is_some_and(|errors| errors.iter().any(|e| e["name"] == "crs-supported-core"));
            }
            if invalid && publication_generations(ctx).await?.iter().any(|g| !before.contains(g)) {
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "invalid supported bundle did not reach Controller/Gateway"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        ctx.wait_phase(SCENARIO, "supported-invalid-retained", Duration::from_secs(60))
            .await?;
        ctx.apply_dir("updates/to_supported_on").await?;
        ctx.wait_phase(SCENARIO, "supported-recovered", Duration::from_secs(60))
            .await?;
        assert_supported_publication(ctx, "strict", false).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let restored = tokio::time::timeout(Duration::from_secs(30), async {
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(15)).await
    })
    .await
    .context("supported restoration timed out")
    .and_then(|r| r);
    let deleted = delete_supported_resources(ctx).await;
    match (result, restored, deleted) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (r, s, d) => anyhow::bail!("supported segment: result={r:?}; restoration={s:?}; cleanup={d:?}"),
    }
}

async fn assert_original_publication(ctx: &ScenarioCtx, detection: bool) -> Result<()> {
    for gateway in [false, true] {
        let resource = plugin_resource(ctx, gateway).await?;
        let waf = &resource["spec"]["waf"];
        ensure!(
            waf["selectedProfile"] == "original",
            "original WAF profile was not published"
        );
        ensure!(
            waf["mode"] == if detection { "detectionOnly" } else { "on" },
            "original WAF mode mismatch"
        );
        let refs = waf["resolvedPolicy"]["profiles"]["original"]["bundleRefs"]
            .as_array()
            .context("original WAF resolved bundle reference missing")?;
        ensure!(
            refs.len() == 1
                && refs[0]["name"] == "waf-rules-original-crs"
                && refs[0]["expectedDigest"] == suites::edgion_plugins::ORIGINAL_DIGEST,
            "original WAF resolved digest pin mismatch"
        );
        ensure!(
            crate::dynamic::status::conditions(&resource)?
                .iter()
                .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True"),
            "original WAF configuration was not accepted"
        );
    }
    Ok(())
}

async fn run_original_segment(ctx: &ScenarioCtx) -> Result<()> {
    let result = async {
        ctx.apply_dir("updates/to_original_on").await?;
        ctx.wait_phase(SCENARIO, "original-on", Duration::from_secs(30)).await?;
        assert_original_publication(ctx, false).await?;
        ctx.apply_dir("updates/to_original_detection").await?;
        ctx.wait_phase(SCENARIO, "original-detection", Duration::from_secs(30))
            .await?;
        assert_original_publication(ctx, true).await?;
        ctx.apply_dir("updates/to_original_on").await?;
        ctx.wait_phase(SCENARIO, "original-restored", Duration::from_secs(30))
            .await?;
        assert_original_publication(ctx, false).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    // Cleanup both operations even if restoration fails; preserve the original assertion error.
    let restored = async {
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(15)).await
    }
    .await;
    let deleted = delete_original_bundle(ctx).await;
    match (result, restored, deleted) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (result, restored, deleted) => anyhow::bail!(
            "original CRS segment failed: result={result:?}; restoration={restored:?}; deletion={deleted:?}"
        ),
    }
}

async fn delete_original_bundle(ctx: &ScenarioCtx) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let identity = tokio::fs::read_to_string(ctx.resolve("delete/original_bundles.txt")).await?;
        ensure!(
            identity.trim() == "EdgionConfigData/edgion-default/waf-rules-original-crs",
            "original bundle cleanup must target only its declared resource"
        );
        let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
        let response = client
            .delete("EdgionConfigData", Some("edgion-default"), "waf-rules-original-crs")
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        edgion_cli::cli::client::handle_response(response).await.map(|_| ())
    })
    .await
    .context("original bundle cleanup timed out")?
}

fn json_v1() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicJsonV1TestSuite)
}

fn json_v2() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicJsonV2TestSuite)
}

fn json_detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicJsonDetectionTestSuite)
}

fn json_zero_work() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicJsonZeroWorkTestSuite)
}

fn bundle_v1() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicBundleV1TestSuite)
}

fn bundle_v2() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicBundleV2TestSuite)
}

fn selector_high() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSelectorHighTestSuite)
}

fn static_selection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicStaticSelectionTestSuite)
}

async fn plugin_resource(ctx: &ScenarioCtx, gateway: bool) -> Result<Value> {
    let (base, prefix) = if gateway {
        (&ctx.gateway_url, "configclient")
    } else {
        (&ctx.controller_url, "configserver")
    };
    let url = format!(
        "{}/{prefix}/EdgionPlugins?namespace=edgion-default&name=waf-dynamic",
        base.trim_end_matches('/')
    );
    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
    let token = if gateway {
        std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok()
    } else {
        ctx.test_ctx.admin_token.clone()
    };
    if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let response: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(response["success"] == true, "WAF resource lookup failed");
    response
        .get("data")
        .filter(|data| data.is_object())
        .cloned()
        .context("WAF resource missing")
}

async fn publication_generations(ctx: &ScenarioCtx) -> Result<Vec<String>> {
    let url = format!("{}/api/v1/debug/store-stats", ctx.gateway_url.trim_end_matches('/'));
    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(response["success"] == true, "WAF publication inventory query failed");
    let generations = response["data"]["plugin_publication_inventory"]["http_generations"]
        .as_array()
        .context("WAF publication generations are missing")?;
    Ok(generations
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| value.starts_with("edgion-default/waf-dynamic@"))
        .map(str::to_string)
        .collect())
}

async fn wait_optional_bundle_warning(ctx: &ScenarioCtx) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(resource) = plugin_resource(ctx, false).await {
            let conditions = crate::dynamic::status::conditions(&resource)?;
            if conditions.iter().any(|condition| {
                condition["type"] == "PartiallyInvalid"
                    && condition["status"] == "True"
                    && condition["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("optional bundleRefs"))
            }) {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("optional WAF bundle did not report PartiallyInvalid=True")
}

async fn assert_tampered_bundle_processed(ctx: &ScenarioCtx, prior_generations: &[String]) -> Result<()> {
    for gateway in [false, true] {
        let resource = plugin_resource(ctx, gateway).await?;
        let errors = resource["spec"]["waf"]["resolutionErrors"]
            .as_array()
            .context("WAF bundle resolution errors are missing")?;
        ensure!(
            errors
                .iter()
                .any(|error| error["name"] == "waf-bundle-v1" && error["kind"] == "invalid"),
            "tampered bundle has not reached {}",
            if gateway { "Gateway" } else { "Controller" }
        );
        let accepted = crate::dynamic::status::conditions(&resource)?
            .iter()
            .find(|condition| condition["type"] == "Accepted")
            .context("WAF Accepted condition is missing")?;
        ensure!(accepted["status"] == "False", "tampered WAF candidate was accepted");
    }
    let current = publication_generations(ctx).await?;
    ensure!(
        current.iter().any(|generation| !prior_generations.contains(generation)),
        "tampered WAF candidate has not reached the Gateway publisher"
    );
    Ok(())
}

async fn wait_tampered_bundle_processed(ctx: &ScenarioCtx, prior_generations: &[String]) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        match assert_tampered_bundle_processed(ctx, prior_generations).await {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(last
        .unwrap_or_else(|| anyhow::anyhow!("tampered WAF bundle was not observed"))
        .context("tampered WAF bundle did not reach Controller and Gateway"))
}

async fn wait_invalid_json_processed(ctx: &ScenarioCtx) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        let result = async {
            for gateway in [false, true] {
                let resource = plugin_resource(ctx, gateway).await?;
                ensure!(
                    resource["spec"]["waf"]["requestBody"]["inspection"] == "prefix",
                    "invalid JSON candidate has not reached {}",
                    if gateway { "Gateway" } else { "Controller" }
                );
                ensure!(
                    resource["spec"]["waf"]["requestBody"]["json"].is_object(),
                    "JSON opt-in was lost"
                );
                let conditions = crate::dynamic::status::conditions(&resource)?;
                ensure!(
                    conditions
                        .iter()
                        .any(|condition| condition["type"] == "Accepted" && condition["status"] == "False"),
                    "invalid full-only JSON configuration was accepted"
                );
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("invalid JSON candidate was not observed")))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "expensive-on",
        factory: expensive_on,
    },
    PhaseSuite {
        name: "expensive-off",
        factory: expensive_off,
    },
    PhaseSuite {
        name: "expensive-default",
        factory: expensive_off,
    },
    PhaseSuite {
        name: "expensive-restored",
        factory: expensive_on,
    },
    PhaseSuite {
        name: "supported-on",
        factory: supported_on,
    },
    PhaseSuite {
        name: "supported-detection",
        factory: supported_detection,
    },
    PhaseSuite {
        name: "supported-restored",
        factory: supported_retained,
    },
    PhaseSuite {
        name: "supported-standard",
        factory: supported_standard,
    },
    PhaseSuite {
        name: "supported-invalid-retained",
        factory: supported_retained,
    },
    PhaseSuite {
        name: "supported-recovered",
        factory: supported_retained,
    },
    PhaseSuite {
        name: "original-on",
        factory: original_on,
    },
    PhaseSuite {
        name: "original-detection",
        factory: original_detection,
    },
    PhaseSuite {
        name: "original-restored",
        factory: original_on,
    },
    PhaseSuite {
        name: "xml-v1",
        factory: xml_v1,
    },
    PhaseSuite {
        name: "xml-v2",
        factory: xml_v2,
    },
    PhaseSuite {
        name: "xml-detection",
        factory: xml_detection,
    },
    PhaseSuite {
        name: "json-v1",
        factory: json_v1,
    },
    PhaseSuite {
        name: "json-invalid-retained",
        factory: json_v1,
    },
    PhaseSuite {
        name: "json-v2",
        factory: json_v2,
    },
    PhaseSuite {
        name: "json-detection",
        factory: json_detection,
    },
    PhaseSuite {
        name: "json-zero-work",
        factory: json_zero_work,
    },
    PhaseSuite {
        name: "json-recreated",
        factory: json_v1,
    },
    PhaseSuite {
        name: "ordinary-body-limit",
        factory: ordinary_body_limit,
    },
    PhaseSuite {
        name: "inspection-continue",
        factory: inspection_continue,
    },
    PhaseSuite {
        name: "inspection-reject",
        factory: inspection_reject,
    },
    PhaseSuite {
        name: "format-prefix",
        factory: format_prefix,
    },
    PhaseSuite {
        name: "format-head",
        factory: format_head,
    },
    PhaseSuite {
        name: "configured-prefix",
        factory: configured_prefix,
    },
    PhaseSuite {
        name: "detection",
        factory: detection,
    },
    PhaseSuite {
        name: "injection",
        factory: injection,
    },
    PhaseSuite {
        name: "control",
        factory: control,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "invalid-retained",
        factory: updated,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
    PhaseSuite {
        name: "bundle-v1",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "bundle-invalid-retained",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "bundle-v2",
        factory: bundle_v2,
    },
    PhaseSuite {
        name: "bundle-rollback",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "selector-low",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "selector-high",
        factory: selector_high,
    },
    PhaseSuite {
        name: "static-selection",
        factory: static_selection,
    },
    PhaseSuite {
        name: "selector-unknown-default",
        factory: bundle_v1,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        delete_original_bundle(&ctx).await?;
        delete_supported_resources(&ctx).await?;
        if std::env::var("EDGION_TEST_K8S_MODE")
            .is_ok_and(|mode| matches!(mode.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
        {
            // Replace prior dynamic API fields that server-side apply may retain.
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
        } else {
            ctx.assert_phase(scenario, "initial").await?;
        }
        ctx.apply_dir("updates/to_v2").await?;
        ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "invalid-retained", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_prefix_1m").await?;
        ctx.wait_phase(scenario, "configured-prefix", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_format_prefix").await?;
        ctx.wait_phase(scenario, "format-prefix", Duration::from_secs(20))
            .await?;
        run_inspection_policy_updates(&ctx, scenario).await?;
        ctx.apply_dir("updates/to_format_head").await?;
        ctx.wait_phase(scenario, "format-head", Duration::from_secs(20)).await?;
        ctx.apply_dir("updates/to_detection").await?;
        ctx.wait_phase(scenario, "detection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_injection").await?;
        ctx.wait_phase(scenario, "injection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_control").await?;
        ctx.wait_phase(scenario, "control", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_json_v1").await?;
        ctx.wait_phase(scenario, "json-v1", Duration::from_secs(20)).await?;
        ctx.apply_dir("updates/to_json_invalid").await?;
        wait_invalid_json_processed(&ctx).await?;
        ctx.wait_phase(scenario, "json-invalid-retained", Duration::from_secs(20))
            .await?;
        ctx.apply_dir("updates/to_json_v1").await?;
        ctx.wait_phase(scenario, "json-v1", Duration::from_secs(20)).await?;
        let paused = suites::edgion_plugins::PausedJsonUpload::start(&ctx.test_ctx)
            .await
            .map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_json_v2").await?;
        // New requests must prove V2 ready while the old upload remains open.
        // Local Continue pinned its V1 transaction before this publication.
        ctx.wait_phase(scenario, "json-v2", Duration::from_secs(20)).await?;
        paused.finish().await.map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_json_detection").await?;
        ctx.wait_phase(scenario, "json-detection", Duration::from_secs(20))
            .await?;
        ctx.apply_dir("updates/to_json_v1").await?;
        ctx.wait_phase(scenario, "json-v1", Duration::from_secs(20)).await?;
        let paused_budget = suites::edgion_plugins::PausedJsonUpload::start(&ctx.test_ctx)
            .await
            .map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_json_zero_work").await?;
        ctx.wait_phase(scenario, "json-zero-work", Duration::from_secs(20))
            .await?;
        // Fresh requests prove the new zero-work budget is enforced even in
        // DetectionOnly. The open V1 request keeps its nonzero budget and On mode.
        paused_budget.finish().await.map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_json_v1").await?;
        ctx.wait_phase(scenario, "json-v1", Duration::from_secs(20)).await?;
        ctx.delete_resources("delete/json_resources.txt").await?;
        ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_json_v1").await?;
        ctx.wait_phase(scenario, "json-recreated", Duration::from_secs(20))
            .await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_xml_v1").await?;
        ctx.wait_phase(scenario, "xml-v1", Duration::from_secs(20)).await?;
        let pinned_xml = suites::edgion_plugins::paused_xml_upload(&ctx.test_ctx)
            .await
            .map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_xml_v2").await?;
        ctx.wait_phase(scenario, "xml-v2", Duration::from_secs(20)).await?;
        pinned_xml.finish().await.map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_xml_v1").await?;
        ctx.wait_phase(scenario, "xml-v1", Duration::from_secs(20)).await?;
        let pinned_mode = suites::edgion_plugins::paused_xml_upload(&ctx.test_ctx)
            .await
            .map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_xml_detection").await?;
        ctx.wait_phase(scenario, "xml-detection", Duration::from_secs(20))
            .await?;
        pinned_mode.finish().await.map_err(anyhow::Error::msg)?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
        run_original_segment(&ctx).await?;
        run_supported_segment(&ctx).await?;
        ctx.delete_resources("delete/xml_bundles.txt").await?;
        ctx.delete_resources("delete/json_bundles.txt").await?;
        ctx.apply_dir("updates/to_bundle_v1").await?;
        ctx.wait_phase(scenario, "bundle-v1", Duration::from_secs(15)).await?;
        let prior_generations = publication_generations(&ctx).await?;
        ensure!(!prior_generations.is_empty(), "bundle V1 has no published generation");
        ctx.apply_dir("updates/to_bundle_tampered").await?;
        wait_tampered_bundle_processed(&ctx, &prior_generations).await?;
        ctx.wait_phase(scenario, "bundle-invalid-retained", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_bundle_v2").await?;
        ctx.wait_phase(scenario, "bundle-v2", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_bundle_rollback").await?;
        ctx.wait_phase(scenario, "bundle-rollback", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_bundle_unpinned_policy").await?;
        ctx.wait_phase(scenario, "bundle-v1", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_bundle_unpinned_update").await?;
        ctx.wait_phase(scenario, "bundle-v2", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_bundle_rollback").await?;
        ctx.wait_phase(scenario, "bundle-rollback", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_initial").await?;
        ctx.wait_phase(scenario, "selector-low", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_inline_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_initial").await?;
        ctx.wait_phase(scenario, "selector-low", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(15))
            .await?;
        wait_optional_bundle_warning(&ctx).await?;
        ctx.apply_dir("updates/to_selector_unknown").await?;
        ctx.wait_phase(scenario, "selector-unknown-default", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(15))
            .await?;
        // Only WafPolicy changes: source identities and digest pins stay fixed.
        let before = plugin_resource(&ctx, true).await?;
        let pins = before["spec"]["waf"]["resolvedPolicy"]["profiles"]["high"]["bundleRefs"].clone();
        let bundles = before["spec"]["waf"]["resolvedBundles"].clone();
        ctx.apply_dir("updates/to_static_selection").await?;
        ctx.wait_phase(scenario, "static-selection", Duration::from_secs(20))
            .await?;
        for gateway in [false, true] {
            let selected = plugin_resource(&ctx, gateway).await?;
            ensure!(
                selected["spec"]["waf"]["resolvedPolicy"]["profiles"]["high"]["bundleRefs"] == pins,
                "selection changed original digest pins"
            );
            ensure!(
                selected["spec"]["waf"]["resolvedBundles"] == bundles,
                "selection changed original resolved bundles"
            );
            ensure!(
                selected["spec"]["waf"]["resolvedPolicy"]["profiles"]["high"]["ruleSelection"]["exclude"].is_object(),
                "selection tree not transported"
            );
        }
        ctx.apply_dir("updates/to_selector_initial").await?;
        ctx.apply_dir("updates/to_selector_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(20))
            .await?;
        ctx.delete_resources("delete/resources.txt").await?;
        ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "WafDynamic",
    conf_path: "EdgionPlugins/Waf/DynamicTest",
    run,
    phases: PHASES,
};

struct ExpensiveChecksSuite(bool);
impl TestSuite for ExpensiveChecksSuite {
    fn name(&self) -> &str {
        "Waf expensive-check configuration"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let enabled = self.0;
        vec![TestCase::new(
            "hpp_policy",
            "pinned EGC optional HPP policy and ordinary attack coverage",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let started = std::time::Instant::now();
                    let run = async {
                        for (query, attack, multipart) in [
                            ("a=one&a=two", false, false),
                            ("probe=phar%3A%2F%2Fpayload", true, false),
                            ("", false, true),
                            ("", true, true),
                        ] {
                            let trace = format!("waf-policy-{}", uuid::Uuid::new_v4());
                            let response = ctx
                                .http_client
                                .post(format!("http://{}:{}/echo?{query}", ctx.target_host, ctx.http_port))
                                .timeout(Duration::from_secs(10))
                                .header("host", "waf-supported.example.com")
                                .header("x-waf-version", "supported-crs")
                                .header("content-type", if multipart { "multipart/form-data; boundary=boundary" } else { "application/json" })
                                .header("user-agent", "edgion-profile-test")
                                .header("access_log", "test_store")
                                .header("traceparent", crate::access_log_client::traceparent_for_marker(&trace))
                                .body(if multipart && attack {
                                    "--boundary\r\nContent-Disposition: form-data; name=\"part\"\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\nsafe\r\n--boundary--\r\n"
                                } else if multipart {
                                    "--boundary\r\nContent-Disposition: form-data; name=\"part\"\r\nContent-Type: invalid\r\n\r\nsafe\r\n--boundary--\r\n"
                                } else { "{}" })
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            let status = response.status().as_u16();
                            response.bytes().await.map_err(|e| e.to_string())?;
                            let log = ctx
                                .access_log_client()
                                .get_access_log_with_retry(&trace, 15, 200)
                                .await
                                .map_err(|e| e.to_string())?;
                            let expected = if attack || enabled { 403 } else { 200 };
                            if status != expected || log.status_code() != Some(u64::from(expected)) {
                                return Err(format!("WAF policy status: actual={status}, expected={expected}"));
                            }
                            let findings = log.data["waf"]["findings"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                            let id = if multipart && attack { 922120 } else if attack { 933200 } else if multipart { 922110 } else { 921180 };
                            if findings.iter().any(|f| f["rule_id"] == id) != (attack || enabled) {
                                return Err(format!("WAF policy finding {id} mismatch: {}", log.data["waf"]));
                            }
                        }
                        Ok::<(), String>(())
                    }
                    .await;
                    match run {
                        Ok(()) => TestResult::passed(started.elapsed()),
                        Err(error) => TestResult::failed(started.elapsed(), error),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(45))]
    }
}
fn expensive_on() -> Box<dyn TestSuite> {
    Box::new(ExpensiveChecksSuite(true))
}
fn expensive_off() -> Box<dyn TestSuite> {
    Box::new(ExpensiveChecksSuite(false))
}

async fn run_expensive_policy_updates(ctx: &ScenarioCtx, scenario: &'static Scenario) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut baseline: Value = client
        .get("EdgionGatewayConfig", None, "example-gateway")
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        baseline["kind"] == "EdgionGatewayConfig",
        "baseline EGC resource missing"
    );
    baseline
        .as_object_mut()
        .context("EGC must be an object")?
        .remove("status");
    if let Some(metadata) = baseline["metadata"].as_object_mut() {
        metadata.remove("resourceVersion");
        metadata.remove("managedFields");
    }
    baseline["spec"]
        .as_object_mut()
        .context("EGC spec must be an object")?
        .remove("currentStatus");
    let apply = |resource: Value| {
        let client = &client;
        async move {
            crate::dynamic::apply::apply_yaml_documents(
                client,
                &ctx.resolve("expensive-check-policy"),
                &serde_yaml::to_string(&resource)?,
            )
            .await
        }
    };
    let result = async {
        for (flag, phase) in [
            (Some(true), "expensive-on"),
            (Some(false), "expensive-off"),
            (None, "expensive-default"),
            (Some(true), "expensive-restored"),
        ] {
            let mut candidate = baseline.clone();
            if let Some(flag) = flag {
                candidate["spec"]["waf"] = serde_json::json!({"expensiveChecks": flag});
            } else {
                candidate["spec"].as_object_mut().unwrap().remove("waf");
            }
            apply(candidate).await?;
            ctx.wait_phase(scenario, phase, Duration::from_secs(30)).await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    // Restore the captured resource even when a phase fails.
    let restoration = apply(baseline).await;
    match (result, restoration) {
        (Ok(()), Ok(())) => Ok(()),
        (result, restoration) => anyhow::bail!("WAF EGC update: result={result:?}; restoration={restoration:?}"),
    }
}

fn run_expensive(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            ctx.apply_dir("updates/to_supported_on").await?;
            ctx.wait_phase(EXPENSIVE_SCENARIO, "supported-recovered", Duration::from_secs(60))
                .await?;
            run_expensive_policy_updates(&ctx, EXPENSIVE_SCENARIO).await
        }
        .await;
        let restored = ctx.apply_dir("updates/to_recovered").await;
        let deleted = delete_supported_resources(&ctx).await;
        match (result, restored, deleted) {
            (Ok(()), Ok(()), Ok(())) => Ok(()),
            (result, restored, deleted) => {
                anyhow::bail!("WAF optional checks: result={result:?}; restore={restored:?}; cleanup={deleted:?}")
            }
        }
    })
}

pub static EXPENSIVE_SCENARIO: &Scenario = &Scenario {
    id: "WafExpensiveChecks",
    conf_path: "EdgionPlugins/Waf/DynamicTest",
    run: run_expensive,
    phases: &[
        PhaseSuite {
            name: "supported-recovered",
            factory: supported_retained,
        },
        PhaseSuite {
            name: "expensive-on",
            factory: expensive_on,
        },
        PhaseSuite {
            name: "expensive-off",
            factory: expensive_off,
        },
        PhaseSuite {
            name: "expensive-default",
            factory: expensive_off,
        },
        PhaseSuite {
            name: "expensive-restored",
            factory: expensive_on,
        },
    ],
};

async fn run_inspection_policy_updates(ctx: &ScenarioCtx, scenario: &'static Scenario) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut baseline: Value = client
        .get("EdgionGatewayConfig", None, "example-gateway")
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        baseline["kind"] == "EdgionGatewayConfig",
        "baseline EGC resource missing"
    );
    baseline
        .as_object_mut()
        .context("EGC must be an object")?
        .remove("status");
    if let Some(metadata) = baseline["metadata"].as_object_mut() {
        metadata.remove("resourceVersion");
        metadata.remove("managedFields");
    }
    baseline["spec"]
        .as_object_mut()
        .context("EGC spec must be an object")?
        .remove("currentStatus");
    let apply = |resource: Value| {
        let client = &client;
        async move {
            // The Controller reads raw Kubernetes objects, while the Gateway
            // publishes the typed resource with canonical serde defaults.
            let body: edgion_resources::resources::RequestBodyConfig = serde_json::from_value(
                resource["spec"]
                    .get("requestBody")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            )?;
            let expected_body = serde_json::to_value(body)?;
            let refs: Option<Vec<edgion_resources::resources::PluginReference>> =
                serde_json::from_value(resource["spec"]["globalPluginsRef"].clone())?;
            let expected_refs = serde_json::to_value(refs)?;
            crate::dynamic::apply::apply_yaml_documents(
                client,
                &ctx.resolve("inspection-storage-policy"),
                &serde_yaml::to_string(&resource)?,
            )
            .await?;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let url = format!(
                        "{}/configclient/EdgionGatewayConfig?name=example-gateway",
                        ctx.gateway_url.trim_end_matches('/')
                    );
                    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
                    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
                        request = request.bearer_auth(token);
                    }
                    let value: Value = request.send().await?.error_for_status()?.json().await?;
                    if value["success"] == true
                        && value["data"]["spec"]["requestBody"] == expected_body
                        && value["data"]["spec"]["globalPluginsRef"] == expected_refs
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
            .context("request body policy publication timed out")??;
            Ok::<(), anyhow::Error>(())
        }
    };
    let result = async {
        let mut inspection = baseline.clone();
        // The shared GlobalPluginsSnapshot fixture has a conservative complete-
        // body consumer. Isolate the early-dispatch probes from that reference;
        // restoration below retains the complete original global reference list.
        if let Some(refs) = inspection["spec"]["globalPluginsRef"].as_array_mut() {
            refs.retain(|reference| {
                !(reference["name"] == "gp-resp-header-modifier" && reference["namespace"] == "edgion-default")
            });
        }
        let mut disk = inspection.clone();
        disk["spec"]["requestBody"] = serde_json::json!({
            "enabled": true, "maxBodySize": "1MiB",
            "storage": { "memoryLimit": "1", "spillToDisk": true, "maxBufferedSize": "32", "operationTimeout": "5s" }
        });
        apply(disk).await?;
        for action in ["continue", "reject"] {
            ctx.apply_dir(&format!("updates/to_inspection_{action}")).await?;
            ctx.wait_phase(scenario, &format!("inspection-{action}"), Duration::from_secs(30))
                .await?;
        }
        let mut ordinary = inspection;
        ordinary["spec"]["requestBody"] = serde_json::json!({
            "enabled": false, "maxBodySize": "128",
            "storage": { "memoryLimit": "128", "spillToDisk": false, "maxBufferedSize": "128", "operationTimeout": "5s" }
        });
        apply(ordinary).await?;
        ctx.wait_phase(scenario, "ordinary-body-limit", Duration::from_secs(30)).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let restoration = apply(baseline).await;
    match (result, restoration) {
        (Ok(()), Ok(())) => Ok(()),
        (result, restoration) => {
            anyhow::bail!("WAF inspection storage update: result={result:?}; restoration={restoration:?}")
        }
    }
}
