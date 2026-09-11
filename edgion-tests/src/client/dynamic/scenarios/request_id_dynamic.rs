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
                        let trace = uuid::Uuid::new_v4().simple().to_string();
                        let response = ctx
                            .http_client
                            .get(format!("{}/headers", ctx.http_url()))
                            .header("host", "request-id-dynamic.example.com")
                            .header("traceparent", format!("00-{trace}-0123456789abcdef-00"))
                            .header("access_log", "test_store")
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        let status = response.status();
                        let headers = response.headers().clone();
                        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
                        let log = ctx
                            .access_log_client()
                            .get_access_log_with_retry(&trace, 30, 50)
                            .await
                            .map_err(|e| e.to_string())?;
                        let ids = &log.data["request_info"]["trace_id"];
                        if ids["x-trace-id"] != trace
                            || !ids
                                .as_object()
                                .is_some_and(|map| map.values().all(serde_json::Value::is_string))
                        {
                            return Err("flat log map must retain W3C identity in every phase".into());
                        }
                        if phase == "deleted" {
                            return if status == 404
                                && headers.get("x-trace-id").and_then(|v| v.to_str().ok()) == Some(trace.as_str())
                                && ids.get("x-dynamic-id").is_none()
                            {
                                Ok(())
                            } else {
                                Err("route must be absent while core default survives".into())
                            };
                        }
                        if status != 200 {
                            return Err(format!("expected 200, got {status}"));
                        }
                        let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        let id = headers
                            .get("x-dynamic-id")
                            .and_then(|v| v.to_str().ok())
                            .ok_or("missing dynamic ID")?;
                        let prefix = if phase == "initial" { "v1-" } else { "v2-" };
                        if !id.starts_with(prefix) || body["headers"]["x-dynamic-id"].as_str() != Some(id) {
                            return Err("generator generation or upstream/response identity differs".into());
                        }
                        if ids["x-dynamic-id"] != id {
                            return Err("custom ID differs between response and access log".into());
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
