//! RequestId resource replacement updates response and upstream state together.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

struct Phase(&'static str);
impl TestSuite for Phase {
    fn name(&self) -> &str {
        "RequestId hot reload"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "request_id_dynamic_wire",
            "Poll final request and response ID behavior",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<(), String> = async {
                        let response = ctx
                            .http_client
                            .get(format!("{}/headers", ctx.http_url()))
                            .header("host", "request-id-dynamic.example.com")
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if phase == "deleted" {
                            return if response.status() == 404 && response.headers().contains_key("x-trace-id") {
                                Ok(())
                            } else {
                                Err("route must be absent while core default survives".into())
                            };
                        }
                        if response.status() != 200 {
                            return Err(format!("expected 200, got {}", response.status()));
                        }
                        let headers = response.headers().clone();
                        let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
                        let id = headers
                            .get("x-dynamic-id")
                            .and_then(|v| v.to_str().ok())
                            .ok_or("missing dynamic ID")?;
                        let prefix = if phase == "initial" { "v1-" } else { "v2-" };
                        if !id.starts_with(prefix) || body["headers"]["x-dynamic-id"].as_str() != Some(id) {
                            return Err("generator generation or upstream/response identity differs".into());
                        }
                        let default_expected = phase == "initial";
                        if headers.contains_key("x-trace-id") != default_expected
                            || body["headers"]["x-trace-id"].is_string() != default_expected
                        {
                            return Err("default removal did not propagate to both directions".into());
                        }
                        if !body["headers"]["traceparent"].is_string() {
                            return Err("W3C context disappeared during update".into());
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )]
    }
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(Phase("initial"))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(Phase("updated"))
}
fn deleted() -> Box<dyn TestSuite> {
    Box::new(Phase("deleted"))
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
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // Establish both resources before deletion, including on a clean manual run.
            ctx.apply_dir("initial/").await?;
            ctx.delete_resources("delete/resources.txt").await?;
            ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(15)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(15)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await
        }
        .await;
        let cleanup = async {
            ctx.delete_resources("delete/resources.txt").await?;
            ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(15)).await
        }
        .await;
        result?;
        cleanup
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "RequestIdDynamic",
    conf_path: "EdgionPlugins/RequestId/DynamicTest",
    run,
    phases: PHASES,
};
