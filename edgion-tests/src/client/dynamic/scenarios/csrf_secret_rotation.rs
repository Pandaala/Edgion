//! CSRF signing-key rotation, deletion, and recovery without process reloads.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestContext, TestSuite};
use crate::suites;
use anyhow::{Context, Result};
use std::time::Duration;

const HOST: &str = "csrf-secret-rotation.example.com";
const TOKEN_HEADER: &str = "X-CSRF-Token";

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::CsrfSecretRotationInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::CsrfSecretRotationUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::CsrfSecretRotationDeletedTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

fn extract_csrf_token(set_cookie: &str, name: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

async fn issue_token(ctx: &TestContext) -> Result<String> {
    let response = ctx
        .http_client
        .get(format!("{}/health", ctx.http_url()))
        .header("host", HOST)
        .send()
        .await
        .context("CSRF pre-rotation issuance request failed")?;
    anyhow::ensure!(
        response.status().as_u16() == 200,
        "CSRF pre-rotation issuance returned {}",
        response.status()
    );
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| extract_csrf_token(cookie, TOKEN_HEADER))
        .context("CSRF pre-rotation issuance response did not include a token cookie")
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.apply_dir("initial/").await?;
        ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
        let _pre_rotation = suites::capture_csrf_pre_rotation_token(issue_token(&ctx.test_ctx).await?);

        let rotation_result = async {
            ctx.apply_dir("updates/00_Secret_csrf-rotation-key.yaml").await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await
        }
        .await;

        let delete_result = async {
            ctx.delete_resources("delete/secret.txt").await?;
            ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await
        }
        .await;

        let restore_result = async {
            ctx.apply_dir("initial/00_Secret_csrf-rotation-key.yaml").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;
        restore_result?;
        rotation_result?;
        delete_result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "CsrfSecretRotation",
    conf_path: "EdgionPlugins/Csrf/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use yaml_serde::Value;

    const INITIAL_SECRET: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integration/conf/EdgionPlugins/Csrf/DynamicTest/initial/00_Secret_csrf-rotation-key.yaml"
    ));
    const UPDATED_SECRET: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integration/conf/EdgionPlugins/Csrf/DynamicTest/updates/00_Secret_csrf-rotation-key.yaml"
    ));

    fn secret_key(manifest: &str) -> String {
        let value: Value = yaml_serde::from_str(manifest).unwrap();
        value["data"]["key"].as_str().unwrap().to_string()
    }

    fn decoded_utf8_key(manifest: &str) -> String {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(secret_key(manifest))
            .expect("CSRF rotation fixture key must be standard base64");
        String::from_utf8(raw).expect("CSRF rotation fixture key must decode to UTF-8")
    }

    #[test]
    fn rotation_fixtures_use_distinct_utf8_keys() {
        let initial = decoded_utf8_key(INITIAL_SECRET);
        let updated = decoded_utf8_key(UPDATED_SECRET);
        assert!(!initial.trim().is_empty());
        assert!(!updated.trim().is_empty());
        assert!(
            initial != updated,
            "CSRF rotation fixtures must use distinct signing keys"
        );
    }
}
