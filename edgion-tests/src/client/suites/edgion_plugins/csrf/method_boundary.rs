use super::csrf::extract_csrf_token;
use crate::framework::{TestCase, TestContext, TestResult};
use crate::port_config::{PortConfig, PortKind};
use reqwest::Method;
use serde_json::Value;

async fn counts(ctx: &TestContext) -> Result<Value, String> {
    let hosts = crate::framework::test_backend_hosts(ctx);
    if hosts.is_empty() {
        return Err("Missing origin hosts".into());
    }
    let mut totals = serde_json::json!({"calls": 0u64, "reads": 0u64, "writes": 0u64});
    for host in hosts {
        let value: Value = ctx
            .http_client
            .get(format!("http://{host}:30001/csrf-method-boundary/observation"))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        for field in ["calls", "reads", "writes"] {
            let count = value[field].as_u64().ok_or("Invalid origin counter")?;
            let total = totals[field].as_u64().ok_or("Invalid counter total")?;
            totals[field] = total.checked_add(count).ok_or("Origin counter overflow")?.into();
        }
    }
    Ok(totals)
}

async fn run(ctx: &TestContext) -> Result<(), String> {
    let port = PortConfig::load()?.require_port("EdgionPlugins/Csrf/MethodBoundary", PortKind::Http)?;
    let base = format!("http://{}:{port}", ctx.target_host);
    // Bootstrap on a separate, genuinely read-only route using the same signing key.
    let bootstrap = ctx
        .http_client
        .get(format!("{}/health", ctx.http_url()))
        .header("host", "csrf-test.example.com")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if bootstrap.status().as_u16() != 200 {
        return Err("CSRF token bootstrap failed".into());
    }
    let token = bootstrap
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|cookie| cookie.to_str().ok())
        .find_map(|cookie| extract_csrf_token(cookie, "X-CSRF-Token"))
        .ok_or("Missing bootstrap CSRF cookie")?;
    bootstrap.bytes().await.map_err(|e| e.to_string())?;

    for (variant, method, valid_token, expected, final_method, effect) in [
        ("rewrite-before", Method::GET, false, 401, "POST", "none"),
        ("rewrite-before", Method::GET, true, 200, "POST", "write"),
        ("csrf-before", Method::GET, false, 200, "POST", "write"),
        ("get-before", Method::POST, false, 200, "GET", "read"),
        ("get-after", Method::POST, false, 401, "GET", "none"),
        ("get-after", Method::POST, true, 200, "GET", "read"),
        ("global-route", Method::GET, false, 200, "POST", "write"),
        ("global-backend", Method::GET, false, 200, "POST", "write"),
        ("gateway-route", Method::GET, false, 200, "POST", "write"),
        ("global-route", Method::POST, false, 401, "POST", "none"),
        ("backend-protected", Method::GET, false, 401, "POST", "none"),
        ("backend-protected", Method::GET, true, 200, "POST", "write"),
        ("query", Method::GET, false, 200, "POST", "read"),
        ("query-rewritten", Method::GET, false, 200, "POST", "read"),
        ("mutation", Method::GET, false, 405, "POST", "none"),
        ("mutation-rewritten", Method::GET, false, 405, "POST", "none"),
        ("mutation", Method::POST, false, 401, "POST", "none"),
        ("mutation", Method::POST, true, 200, "POST", "write"),
    ] {
        let before = counts(ctx).await?;
        let mut request = ctx
            .http_client
            .request(method.clone(), format!("{base}/entry"))
            .header("host", format!("csrf-boundary-{variant}.example.com"))
            .header(
                "cookie",
                if valid_token {
                    format!("fixture-session=authenticated; X-CSRF-Token={token}")
                } else {
                    "fixture-session=authenticated".to_owned()
                },
            );
        if valid_token {
            request = request.header("X-CSRF-Token", &token);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        // Drain before observing origin counters, including local rejections.
        let value: Value = response.json().await.map_err(|e| e.to_string())?;
        let after = counts(ctx).await?;
        let allowed = expected == 200;
        let graphql = matches!(variant, "query" | "query-rewritten" | "mutation" | "mutation-rewritten");
        let path = if graphql {
            "/csrf-method-boundary/graphql"
        } else {
            "/csrf-method-boundary/action"
        };
        if status != expected {
            return Err(format!(
                "{variant} {method} token={valid_token}: expected {expected}, got {status}"
            ));
        }
        for (field, delta) in [
            ("calls", u64::from(allowed)),
            ("reads", u64::from(effect == "read")),
            ("writes", u64::from(effect == "write")),
        ] {
            let old = before[field].as_u64().ok_or("Invalid origin counter")?;
            if after[field].as_u64() != Some(old + delta) {
                return Err(format!("{variant}: wrong origin {field} delta (expected {delta})"));
            }
        }
        if allowed && (value["method"] != final_method || value["path"] != path || value["effect"] != effect) {
            return Err(format!("{variant}: wrong final method, path or origin effect"));
        }
    }
    Ok(())
}

pub(super) fn test_case() -> TestCase {
    TestCase::new(
        "method_rewrite_protection_boundary",
        "Real scoped chains preserve execution-time CSRF checks and GraphQL entry-method admission",
        |ctx: TestContext| {
            Box::pin(async move {
                let start = std::time::Instant::now();
                match run(&ctx).await {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}
