use std::sync::Arc;
use std::time::Duration;

use serde_json::{self, Value};
use std::borrow::Borrow;
use tokio::sync::Mutex;
use tokio::task::yield_now;
use tokio::time::timeout;

use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::config_server::EventDataSimple;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::core::conf_sync::{ConfigServer, EventDispatch};
use crate::types::{GatewayClass, GatewayClassSpec, ResourceKind};

fn sample_gateway_class(name: &str, version: u64) -> GatewayClass {
    let mut gc = GatewayClass::new(
        name,
        GatewayClassSpec {
            controller_name: "edgion.dev/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
    );
    gc.metadata.resource_version = Some(version.to_string());
    gc
}

#[tokio::test(flavor = "multi_thread")]
async fn config_server_and_client_stay_in_sync_via_watch() {
    let key = "gateway-class-test".to_string();

    // Step 1: build a config server with an initialized gateway class cache
    let mut server = ConfigServer::new();
    server
        .gateway_classes
        .write().unwrap()
        .insert(key.clone(), ServerCache::new(32));
    server
        .gateway_classes
        .write().unwrap()
        .get_mut(&key)
        .expect("cache exists")
        .set_ready();

    let mut watch_rx = server
        .watch(
            &key,
            &ResourceKind::GatewayClass,
            "test-client".to_string(),
            "config-test".to_string(),
            0,
        )
        .expect("watch receiver");

    // Step 2: create a config client (hub) that will receive server events
    let client = Arc::new(Mutex::new(ConfigClient::new(key.clone())));
    let client_for_watch = client.clone();

    // Step 3: spawn a task to consume watch events and apply them to the client
    let watcher_task = tokio::spawn(async move {
        if let Ok(Some(event_data)) = timeout(Duration::from_secs(2), watch_rx.recv()).await {
            let events: Vec<Value> =
                serde_json::from_str(&event_data.data).expect("valid watcher events");

            for event in events {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .expect("event type");

                let change = match event_type {
                    "add" => ResourceChange::EventAdd,
                    "update" => ResourceChange::EventUpdate,
                    "delete" => ResourceChange::EventDelete,
                    other => panic!("unexpected event type {}", other),
                };

                let payload = serde_json::to_string(event.get("data").expect("event data"))
                    .expect("serialize watcher event data");

                let resource_version = event
                    .get("resource_version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(event_data.resource_version);

                client_for_watch.lock().await.apply_resource_change(
                    change,
                    Some(ResourceKind::GatewayClass),
                    payload,
                    Some(resource_version),
                );
            }
        } else {
            panic!("timed out waiting for watcher events");
        }
    });

    // Step 4: emit an add event on the server
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let gc = sample_gateway_class(&key, 1);
    let payload = serde_json::to_string(&gc).expect("serialize gateway class");
    server.apply_resource_change(
        ResourceChange::EventAdd,
        Some(ResourceKind::GatewayClass),
        payload,
        Some(1),
    );

    // Step 5: wait for the watcher task to finish applying the event
    watcher_task.await.expect("watcher task completed");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Step 6: compare server snapshot with client state
    let server_snapshot = server
        .list_gateway_classes(&key)
        .expect("server snapshot");
    let client_guard = client.lock().await;
    let client_snapshot = client_guard.list_gateway_classes();

    assert_eq!(
        server_snapshot.resource_version,
        client_snapshot.resource_version
    );

    let server_names: Vec<_> = server_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();
    let client_names: Vec<_> = client_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();

    assert_eq!(server_names, client_names);
}

#[tokio::test(flavor = "current_thread")]
async fn config_client_stays_consistent_during_long_watch_window() {
    let key = "gateway-class-long-run".to_string();

    let use_realtime = std::env::var_os("EDGEION_WATCH_TEST_REALTIME").is_some();
    if !use_realtime {
        tokio::time::pause();
    }

    // Initialize server cache and start watch stream
    let mut server = ConfigServer::new();
    server
        .gateway_classes
        .write().unwrap()
        .insert(key.clone(), ServerCache::new(64));
    server
        .gateway_classes
        .write().unwrap()
        .get_mut(&key)
        .expect("cache exists")
        .set_ready();

    let mut watch_rx = server
        .watch(
            &key,
            &ResourceKind::GatewayClass,
            "long-client".to_string(),
            "config-test".to_string(),
            0,
        )
        .expect("watch receiver");

    let client = Arc::new(Mutex::new(ConfigClient::new(key.clone())));
    let client_for_watch = client.clone();

    // Watcher task: consume events for 30 seconds, applying them to the client cache
    let watcher_task = tokio::spawn(async move {
        let watch_deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(watch_deadline);

        loop {
            tokio::select! {
                _ = &mut watch_deadline => {
                    break;
                }
                maybe_event = watch_rx.recv() => {
                    let event_data = match maybe_event {
                        Some(data) => data,
                        None => break,
                    };

                    if let Some(err) = &event_data.err {
                        panic!("unexpected watch error: {err}");
                    }

                    if event_data.data.is_empty() {
                        continue;
                    }

                    let events: Vec<Value> =
                        serde_json::from_str(&event_data.data).expect("valid watcher events");

                    for event in events {
                        let event_type = event
                            .get("type")
                            .and_then(|v| v.as_str())
                            .expect("event type");

                        let change = match event_type {
                            "add" => ResourceChange::EventAdd,
                            "update" => ResourceChange::EventUpdate,
                            "delete" => ResourceChange::EventDelete,
                            other => panic!("unexpected event type {}", other),
                        };

                        let payload = serde_json::to_string(event.get("data").expect("event data"))
                            .expect("serialize watcher event data");

                        let resource_version = event
                            .get("resource_version")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(event_data.resource_version);

                        client_for_watch.lock().await.apply_resource_change(
                            change,
                            Some(ResourceKind::GatewayClass),
                            payload,
                            Some(resource_version),
                        );
                    }
                }
            }
        }
    });

    tokio::task::yield_now().await;

    // Server emits a predetermined sequence of changes over 25 seconds
    let mut current_version = 1u64;
    let initial_resources = [
        ("alpha", "initial alpha"),
        ("beta", "initial beta"),
        ("gamma", "initial gamma"),
    ];

    for (name, description) in initial_resources {
        let mut gc = sample_gateway_class(name, current_version);
        gc.spec.description = Some(description.to_string());

        let payload = serde_json::to_string(&gc).expect("serialize gateway class");
        server.apply_resource_change(
            ResourceChange::EventAdd,
            Some(ResourceKind::GatewayClass),
            payload,
            Some(current_version),
        );

        current_version += 1;
        wait(Duration::from_secs(1), use_realtime).await;
    }

    enum Mutation<'a> {
        Add { name: &'a str, description: &'a str },
        Update { name: &'a str, description: &'a str },
        Delete { name: &'a str },
    }

    let scheduled_mutations = [
        (
            Duration::from_secs(5),
            Mutation::Update {
                name: "alpha",
                description: "alpha rev-1",
            },
        ),
        (Duration::from_secs(10), Mutation::Delete { name: "beta" }),
        (
            Duration::from_secs(15),
            Mutation::Add {
                name: "delta",
                description: "late delta",
            },
        ),
        (
            Duration::from_secs(20),
            Mutation::Update {
                name: "gamma",
                description: "gamma rev-2",
            },
        ),
        (Duration::from_secs(25), Mutation::Delete { name: "delta" }),
    ];

    let mut last_mark = Duration::from_secs(initial_resources.len() as u64);

    for (target, mutation) in scheduled_mutations {
        if target > last_mark {
            wait(target - last_mark, use_realtime).await;
        }

        match mutation {
            Mutation::Add { name, description } => {
                let mut gc = sample_gateway_class(name, current_version);
                gc.spec.description = Some(description.to_string());

                let payload = serde_json::to_string(&gc).expect("serialize gateway class");
                server.apply_resource_change(
                    ResourceChange::EventAdd,
                    Some(ResourceKind::GatewayClass),
                    payload,
                    Some(current_version),
                );
            }
            Mutation::Update { name, description } => {
                let mut gc = sample_gateway_class(name, current_version);
                gc.spec.description = Some(description.to_string());

                let payload = serde_json::to_string(&gc).expect("serialize gateway class");
                server.apply_resource_change(
                    ResourceChange::EventUpdate,
                    Some(ResourceKind::GatewayClass),
                    payload,
                    Some(current_version),
                );
            }
            Mutation::Delete { name } => {
                let gc = sample_gateway_class(name, current_version);
                let payload = serde_json::to_string(&gc).expect("serialize gateway class");
                server.apply_resource_change(
                    ResourceChange::EventDelete,
                    Some(ResourceKind::GatewayClass),
                    payload,
                    Some(current_version),
                );
            }
        }

        current_version += 1;
        last_mark = target;
    }

    // Allow the watch window to reach 30 seconds total
    if last_mark < Duration::from_secs(30) {
        wait(Duration::from_secs(30) - last_mark, use_realtime).await;
    }

    watcher_task.await.expect("watcher completed");

    wait(Duration::from_millis(50), use_realtime).await;

    let server_snapshot = server
        .list_gateway_classes(&key)
        .expect("server snapshot");
    let client_guard = client.lock().await;
    let client_snapshot = client_guard.list_gateway_classes();

    assert_eq!(
        server_snapshot.resource_version,
        client_snapshot.resource_version
    );

    let mut server_names: Vec<_> = server_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();
    let mut client_names: Vec<_> = client_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();

    server_names.sort();
    client_names.sort();

    assert_eq!(server_names, client_names);
}

async fn wait(duration: Duration, use_realtime: bool) {
    if use_realtime {
        tokio::time::sleep(duration).await;
    } else {
        tokio::time::advance(duration).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_clients_relist_after_stale_watch_error() {
    let key = "gateway-class-multi-client".to_string();
    let use_realtime = std::env::var_os("EDGEION_WATCH_TEST_REALTIME").is_some();
    if !use_realtime {
        tokio::time::pause();
    }

    let server = Arc::new(Mutex::new(ConfigServer::new()));
    {
        let guard = server.lock().await;
        guard
            .gateway_classes
            .write().unwrap()
            .insert(key.clone(), ServerCache::new(16));
        guard
            .gateway_classes
            .write().unwrap()
            .get_mut(&key)
            .expect("cache exists")
            .set_ready();
    }

    let fast_client = Arc::new(Mutex::new(ConfigClient::new(key.clone())));
    let fast_watch_rx = {
        let mut guard = server.lock().await;
        guard
            .watch(
                &key,
                &ResourceKind::GatewayClass,
                "fast-client".to_string(),
                "config-test".to_string(),
                0,
            )
            .expect("fast watch receiver")
    };

    let fast_task = spawn_watch_consumer(
        fast_watch_rx,
        fast_client.clone(),
        Duration::from_secs(3),
        use_realtime,
    );

    let initial_event_count: u64 = 30;
    for version in 1..=initial_event_count {
        let mut gc = sample_gateway_class(&key, version);
        gc.spec.description = Some(format!("initial-{version}"));

        let payload = serde_json::to_string(&gc).expect("serialize gateway class");
        {
            let guard = server.lock().await;
            guard.apply_resource_change(
                ResourceChange::EventAdd,
                Some(ResourceKind::GatewayClass),
                payload,
                Some(version),
            );
        }
        wait(Duration::from_millis(2), use_realtime).await;
    }

    wait(Duration::from_millis(20), use_realtime).await;

    let mut initial_snapshot = {
        let guard = server.lock().await;
        guard
            .list_gateway_classes(&key)
            .expect("initial server snapshot")
    };
    while initial_snapshot.resource_version < initial_event_count {
        wait(Duration::from_millis(5), use_realtime).await;
        yield_now().await;
        initial_snapshot = {
            let guard = server.lock().await;
            guard
                .list_gateway_classes(&key)
                .expect("initial server snapshot")
        };
    }
    let mut latest_version = initial_snapshot.resource_version;
    let stale_from_version = latest_version.saturating_sub(20).max(1);

    let mut stale_watch_rx = {
        let mut guard = server.lock().await;
        guard
            .watch(
                &key,
                &ResourceKind::GatewayClass,
                "stale-client".to_string(),
                "config-test".to_string(),
                stale_from_version,
            )
            .expect("stale watch receiver")
    };

    let stale_client = Arc::new(Mutex::new(ConfigClient::new(key.clone())));

    let error_event = stale_watch_rx
        .recv()
        .await
        .expect("stale watcher should emit an error");
    let err_kind = error_event
        .err
        .as_deref()
        .expect("stale watcher should set err field");
    assert!(
        matches!(err_kind, "TooOldVersion" | "VersionUnexpect"),
        "unexpected watch error: {err_kind}"
    );

    drop(stale_watch_rx);

    let snapshot = {
        let guard = server.lock().await;
        guard
            .list(&key, &ResourceKind::GatewayClass)
            .expect("server list")
    };

    latest_version = snapshot.resource_version;
    let snapshot_items: Vec<GatewayClass> =
        serde_json::from_str(&snapshot.data).expect("decode list snapshot");
    replace_client_with_snapshot(&stale_client, &key, snapshot_items).await;

    let follow_watch_rx = {
        let mut guard = server.lock().await;
        guard
            .watch(
                &key,
                &ResourceKind::GatewayClass,
                "stale-client-follow".to_string(),
                "config-test".to_string(),
                latest_version,
            )
            .expect("follow-up watch receiver")
    };

    let follow_task = spawn_watch_consumer(
        follow_watch_rx,
        stale_client.clone(),
        Duration::from_secs(3),
        use_realtime,
    );

    for offset in 1..=5 {
        latest_version += 1;
        let mut gc = sample_gateway_class(&key, latest_version);
        gc.spec.description = Some(format!("extra-{offset}"));
        let payload = serde_json::to_string(&gc).expect("serialize gateway class");
        {
            let guard = server.lock().await;
            guard.apply_resource_change(
                ResourceChange::EventAdd,
                Some(ResourceKind::GatewayClass),
                payload,
                Some(latest_version),
            );
        }
        wait(Duration::from_millis(2), use_realtime).await;
    }

    wait(Duration::from_secs(1), use_realtime).await;
    wait(Duration::from_secs(3), use_realtime).await;

    fast_task.await.expect("fast watcher task completed");
    follow_task
        .await
        .expect("stale watcher follow-up task completed");

    let server_snapshot = {
        let guard = server.lock().await;
        guard
            .list_gateway_classes(&key)
            .expect("server snapshot")
    };

    let (server_versions, fast_versions, stale_versions) = {
        let server_versions = collect_versions(server_snapshot.data.iter());

        let (fast_versions, fast_version) = {
            let guard = fast_client.lock().await;
            let snapshot = guard.list_gateway_classes();
            assert_eq!(server_snapshot.resource_version, snapshot.resource_version);
            (
                collect_versions(snapshot.data.iter().cloned()),
                snapshot.resource_version,
            )
        };

        let (stale_versions, stale_version) = {
            let guard = stale_client.lock().await;
            let snapshot = guard.list_gateway_classes();
            assert_eq!(server_snapshot.resource_version, snapshot.resource_version);
            (
                collect_versions(snapshot.data.iter().cloned()),
                snapshot.resource_version,
            )
        };

        assert_eq!(fast_version, server_snapshot.resource_version);
        assert_eq!(stale_version, server_snapshot.resource_version);

        (server_versions, fast_versions, stale_versions)
    };

    assert_eq!(server_versions, fast_versions);
    assert_eq!(server_versions, stale_versions);
}

async fn replace_client_with_snapshot(
    client: &Arc<Mutex<ConfigClient>>,
    key: &str,
    items: Vec<GatewayClass>,
) {
    let mut guard = client.lock().await;
    *guard = ConfigClient::new(key.to_string());

    for item in items {
        let resource_version = item
            .metadata
            .resource_version
            .as_deref()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap_or(0);
        let payload = serde_json::to_string(&item).expect("serialize gateway class");
        guard.apply_resource_change(
            ResourceChange::EventAdd,
            Some(ResourceKind::GatewayClass),
            payload,
            Some(resource_version),
        );
    }
}

async fn apply_watch_events_to_client(client: &Arc<Mutex<ConfigClient>>, event: &EventDataSimple) {
    if event.data.is_empty() {
        return;
    }

    let raw_events: Vec<Value> =
        serde_json::from_str(&event.data).expect("valid watcher events payload");

    let mut parsed_events = Vec::with_capacity(raw_events.len());
    for raw in raw_events {
        let event_type = raw
            .get("type")
            .and_then(|v| v.as_str())
            .expect("event type");
        let change = match event_type {
            "add" => ResourceChange::EventAdd,
            "update" => ResourceChange::EventUpdate,
            "delete" => ResourceChange::EventDelete,
            other => panic!("unexpected event type {}", other),
        };

        let payload_value = raw.get("data").expect("event data");
        let payload =
            serde_json::to_string(payload_value).expect("serialize watcher event payload");

        let resource_version = raw
            .get("resource_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(event.resource_version);

        parsed_events.push((change, payload, resource_version));
    }

    let guard = client.lock().await;
    for (change, payload, resource_version) in parsed_events {
        guard.apply_resource_change(
            change,
            Some(ResourceKind::GatewayClass),
            payload,
            Some(resource_version),
        );
    }
}

fn spawn_watch_consumer(
    mut rx: tokio::sync::mpsc::Receiver<EventDataSimple>,
    client: Arc<Mutex<ConfigClient>>,
    duration: Duration,
    use_realtime: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = tokio::time::sleep(duration);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                _ = &mut deadline => break,
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            assert!(event.err.is_none(), "unexpected watcher error: {:?}", event.err);
                            apply_watch_events_to_client(&client, &event).await;
                        }
                        None => break,
                    }
                }
            }
        }

        wait(Duration::from_millis(1), use_realtime).await;
    })
}

fn collect_versions<T>(data: T) -> Vec<u64>
where
    T: IntoIterator,
    T::Item: Borrow<GatewayClass>,
{
    let mut versions: Vec<u64> = data
        .into_iter()
        .filter_map(|item| {
            let gc = item.borrow();
            gc.metadata
                .resource_version
                .as_deref()
                .unwrap_or("0")
                .parse::<u64>()
                .ok()
        })
        .collect();
    versions.sort_unstable();
    versions
}
