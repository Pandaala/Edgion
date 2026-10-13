//! Cleanup result ownership for the three AI backend transition scenarios.

use crate::dynamic::ScenarioCtx;
use anyhow::Result;
use reqwest::StatusCode;
use std::future::Future;
use std::time::Duration;

type ResourceKey<'a> = (&'a str, &'a str, &'a str);

pub(super) async fn cleanup_resources(ctx: &ScenarioCtx, keys: &[ResourceKey<'_>]) -> Result<()> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build();
    let base = ctx.test_ctx.admin_api_url();
    delete_all(keys, |(kind, namespace, name)| {
        let client = &client;
        let url = format!("{base}/api/v1/namespaced/{kind}/{namespace}/{name}");
        async move {
            let client = client.as_ref().map_err(|_| "client construction failed")?;
            let mut request = client.delete(url);
            if let Some(token) = &ctx.test_ctx.admin_token {
                request = request.bearer_auth(token);
            }
            // Neither response bodies nor transport errors are safe diagnostics:
            // they can contain credentials or the configured Admin URL.
            request
                .send()
                .await
                .map(|response| response.status())
                .map_err(|_| "request failed")
        }
    })
    .await
}

async fn delete_all<'a, F, Fut>(keys: &[ResourceKey<'a>], mut delete: F) -> Result<()>
where
    F: FnMut(ResourceKey<'a>) -> Fut,
    Fut: Future<Output = std::result::Result<StatusCode, &'static str>>,
{
    let mut failures = Vec::new();
    for &(kind, namespace, name) in keys {
        let failure = match delete((kind, namespace, name)).await {
            Ok(status) if status.is_success() || status == StatusCode::NOT_FOUND => continue,
            Ok(status) => format!("HTTP {}", status.as_u16()),
            Err(reason) => reason.to_owned(),
        };
        failures.push(format!("delete {kind}/{namespace}/{name}: {failure}"));
    }
    anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}

pub(super) fn combine_outcomes<const N: usize>(outcomes: [(&str, Result<()>); N]) -> Result<()> {
    let failures: Vec<_> = outcomes
        .into_iter()
        .filter_map(|(phase, result)| result.err().map(|error| format!("{phase}: {error:#}")))
        .collect();
    anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}

// Keep initial barriers and business assertions separate so a failed business
// phase still gives final cleanup its successfully captured baseline.
pub(super) async fn run_with_cleanup<T, F, Fut>(
    initial: impl Future<Output = Result<T>>,
    business: impl Future<Output = Result<()>>,
    cleanup: F,
) -> Result<()>
where
    F: FnOnce(Option<T>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let (baseline, scenario) = match initial.await {
        Ok(baseline) => (Some(baseline), business.await),
        Err(error) => (None, Err(error.context("initial cleanup and observations"))),
    };
    let final_cleanup = cleanup(baseline).await;
    combine_outcomes([("scenario", scenario), ("final cleanup", final_cleanup)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn deletion_accepts_success_and_absence_and_attempts_every_key_after_failures() {
        let keys = [
            ("Secret", "test", "absent"),
            ("Secret", "test", "deleted"),
            ("Secret", "test", "denied"),
            ("HTTPRoute", "test", "unreachable"),
            ("HTTPRoute", "test", "last"),
        ];
        let mut attempted = Vec::new();
        let error = delete_all(&keys, |key| {
            attempted.push(key);
            std::future::ready(match key.2 {
                "absent" => Ok(StatusCode::NOT_FOUND),
                "denied" => Ok(StatusCode::FORBIDDEN),
                "unreachable" => Err("request failed"),
                _ => Ok(StatusCode::NO_CONTENT),
            })
        })
        .await
        .unwrap_err();
        assert_eq!(attempted, keys);
        assert_eq!(
            error.to_string(),
            "delete Secret/test/denied: HTTP 403; delete HTTPRoute/test/unreachable: request failed"
        );
        delete_all(&keys, |_| std::future::ready(Ok(StatusCode::NOT_FOUND)))
            .await
            .unwrap();
        delete_all(&keys, |_| std::future::ready(Ok(StatusCode::NO_CONTENT)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn initial_failure_skips_business_but_still_attempts_final_cleanup_without_baseline() {
        let business_ran = Cell::new(false);
        let cleanup_ran = Cell::new(false);
        let error = run_with_cleanup(
            async { anyhow::bail!("initial deletion rejected") },
            async {
                business_ran.set(true);
                Ok(())
            },
            |baseline: Option<u64>| {
                assert_eq!(baseline, None);
                cleanup_ran.set(true);
                std::future::ready(Err(anyhow::anyhow!("final deletion rejected")))
            },
        )
        .await
        .unwrap_err();
        assert!(!business_ran.get());
        assert!(cleanup_ran.get());
        let error = error.to_string();
        assert!(error.contains("scenario: initial cleanup and observations: initial deletion rejected"));
        assert!(error.contains("final cleanup: final deletion rejected"));
    }

    #[tokio::test]
    async fn business_failure_retains_baseline_and_all_final_outcomes() {
        let error = run_with_cleanup(
            async { Ok(7_u64) },
            async { anyhow::bail!("traffic matrix failed") },
            |baseline| async move {
                assert_eq!(baseline, Some(7));
                combine_outcomes([
                    ("deletion", Err(anyhow::anyhow!("Secret delete rejected"))),
                    ("backend A absence", Ok(())),
                    ("backend B absence", Ok(())),
                    (
                        "store-count convergence",
                        Err(anyhow::anyhow!("count exceeded baseline")),
                    ),
                ])
            },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("scenario: traffic matrix failed"));
        assert!(error.contains("final cleanup: deletion: Secret delete rejected"));
        assert!(error.contains("store-count convergence: count exceeded baseline"));
    }

    #[tokio::test]
    async fn successful_business_still_fails_on_non_backend_deletion_with_successful_observations() {
        let result = run_with_cleanup(async { Ok(0) }, async { Ok(()) }, |baseline| async move {
            assert_eq!(baseline, Some(0));
            combine_outcomes([
                ("deletion", Err(anyhow::anyhow!("Secret delete rejected"))),
                ("backend A absence", Ok(())),
                ("backend B absence", Ok(())),
                ("store-count convergence", Ok(())),
            ])
        })
        .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "final cleanup: deletion: Secret delete rejected"
        );
        run_with_cleanup(async { Ok(()) }, async { Ok(()) }, |_| async { Ok(()) })
            .await
            .unwrap();
    }
}
