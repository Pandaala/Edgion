//! Background task that periodically snapshots RegionRoute PluginMetaData from online
//! controllers into SQLite, and keeps the controllers table current.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::db::CenterDb;
use crate::core::center::proxy::ProxyForwarder;

// DTO matching the controller's PluginMetaData list responses
#[derive(serde::Deserialize)]
struct CtrlListResp {
    success: bool,
    data: Option<Vec<serde_json::Value>>,
}

pub async fn run_sync_task(
    aggregator: Arc<ResourceAggregator>,
    proxy: Arc<ProxyForwarder>,
    db: Arc<CenterDb>,
    interval_secs: u64,
    mut shutdown: watch::Receiver<bool>,
) {
    let interval = std::time::Duration::from_secs(interval_secs);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }

        let summaries = aggregator.controller_summaries();

        // Batch all controller upserts in a single spawn_blocking to avoid per-controller lock overhead
        let db2 = db.clone();
        let ctrl_updates: Vec<(String, String, Vec<String>, Vec<String>, bool)> = summaries
            .iter()
            .map(|s| {
                (
                    s.controller_id.clone(),
                    s.cluster.clone(),
                    s.env.clone(),
                    s.tag.clone(),
                    s.online,
                )
            })
            .collect();
        match tokio::task::spawn_blocking(move || {
            for (id, cluster, env, tag, online) in &ctrl_updates {
                db2.upsert_controller(id, cluster, env, tag, *online)?;
            }
            Ok::<_, rusqlite::Error>(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(component = "center-sync", error = %e, "Failed to upsert controllers"),
            Err(e) => {
                tracing::warn!(component = "center-sync", error = %e, "Spawn blocking panicked on controller upserts")
            }
        }

        // Sync PluginMetaData snapshots from online controllers
        let online: Vec<_> = summaries.into_iter().filter(|s| s.online).collect();

        let futs = online.iter().map(|s| {
            let proxy = proxy.clone();
            let db = db.clone();
            let controller_id = s.controller_id.clone();
            async move {
                sync_one_controller(&proxy, &db, &controller_id).await;
            }
        });

        futures::future::join_all(futs).await;
    }
}

pub(crate) async fn sync_one_controller(proxy: &ProxyForwarder, db: &CenterDb, controller_id: &str) {
    sync_plugin_metadata_list(
        proxy,
        db,
        controller_id,
        "/api/v1/cluster-region-pms",
        "cluster_plugin_metadata_cache",
    )
    .await;
    sync_plugin_metadata_list(
        proxy,
        db,
        controller_id,
        "/api/v1/service-region-pms",
        "service_plugin_metadata_cache",
    )
    .await;
}

async fn sync_plugin_metadata_list(
    proxy: &ProxyForwarder,
    db: &CenterDb,
    controller_id: &str,
    path: &str,
    cache_table: &str,
) {
    let result = proxy
        .forward(
            controller_id,
            "GET".to_string(),
            path.to_string(),
            HashMap::new(),
            vec![],
        )
        .await;

    let body = match result {
        Ok(r) if r.status_code == 200 => r.body,
        Ok(r) => {
            tracing::debug!(
                component = "center-sync",
                controller_id,
                path,
                status = r.status_code,
                "PluginMetaData list non-200, skip"
            );
            return;
        }
        Err((_, e)) => {
            tracing::debug!(
                component = "center-sync",
                controller_id,
                path,
                error = %e,
                "PluginMetaData list failed, skip"
            );
            return;
        }
    };

    let resp: CtrlListResp = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                component = "center-sync",
                controller_id,
                path,
                error = %e,
                "Failed to parse PluginMetaData list"
            );
            return;
        }
    };

    if !resp.success {
        return;
    }

    let entries = resp.data.unwrap_or_default();
    let entry_count = entries.len();

    let db2 = db.clone();
    let ctrl_id = controller_id.to_string();
    let table = cache_table.to_string();
    match tokio::task::spawn_blocking(move || {
        for entry in &entries {
            let ns = entry["namespace"].as_str().unwrap_or("");
            let name = entry["name"].as_str().unwrap_or("");
            let json = serde_json::to_string(entry).unwrap_or_default();
            db2.upsert_plugin_metadata_cache(&table, &ctrl_id, ns, name, &json)?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(component = "center-sync", controller_id, error = %e, "Failed to upsert PluginMetaData cache")
        }
        Err(e) => {
            tracing::warn!(component = "center-sync", controller_id, error = %e, "Spawn blocking panicked on PluginMetaData cache upserts")
        }
    }

    tracing::debug!(
        component = "center-sync",
        controller_id,
        path,
        entries = entry_count,
        "Synced PluginMetaData snapshot"
    );
}
