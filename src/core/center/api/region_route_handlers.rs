//! Shared helpers for Center aggregation handlers.

use std::collections::HashMap;

use super::ApiState;

/// Forward a GET request to all online controllers in parallel, parse each
/// response as `T`, and return `(controller_id, parsed)` pairs.
/// Controllers that fail or return unparseable responses are skipped with a warning.
pub(crate) async fn forward_to_all_online<T: serde::de::DeserializeOwned>(
    state: &ApiState,
    path: String,
    endpoint_label: &str,
) -> Vec<(String, T)> {
    let summaries = state.aggregator.controller_summaries();
    let online: Vec<_> = summaries.into_iter().filter(|s| s.online).collect();

    let futs = online.iter().map(|s| {
        let proxy = state.proxy.clone();
        let controller_id = s.controller_id.clone();
        let path = path.clone();
        async move {
            let result = proxy
                .forward(&controller_id, "GET".to_string(), path, HashMap::new(), vec![])
                .await;
            (controller_id, result)
        }
    });

    let results = futures::future::join_all(futs).await;
    let mut parsed_results = Vec::new();

    for (controller_id, result) in results {
        let resp = match result {
            Ok(r) if r.status_code == 200 => r,
            Ok(r) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id,
                    status = r.status_code,
                    "Non-OK response for {} from controller, skipping", endpoint_label
                );
                continue;
            }
            Err((status, msg)) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id,
                    status = %status, error = %msg,
                    "Failed to fetch {} from controller, skipping", endpoint_label
                );
                continue;
            }
        };

        match serde_json::from_slice::<T>(&resp.body) {
            Ok(v) => parsed_results.push((controller_id, v)),
            Err(e) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id, error = %e,
                    "Failed to parse {} response from controller, skipping", endpoint_label
                );
            }
        }
    }

    parsed_results
}
