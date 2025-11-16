use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use kube::{Resource, ResourceExt};
use tokio::sync::{mpsc, Notify};

use super::store::EventStore;
use super::traits::{EventDispatch, Versionable};
use super::types::{EventType, ListData, WatchClient, WatchResponse};
use crate::core::conf_sync::traits::ResourceChange;

pub struct ServerCache<T: Versionable + Resource + Send + Sync> {
    // wait for init complete
    ready: RwLock<bool>,

    // Event storage
    store: Arc<RwLock<EventStore<T>>>,

    // pending watch requests
    watchers: Vec<WatchClient<T>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: Versionable + Resource + Send + Sync> ServerCache<T> {
    pub fn new(capacity: u32) -> Self
    where
        T: Clone,
    {
        Self {
            ready: RwLock::new(false),
            store: Arc::new(RwLock::new(EventStore::new(capacity as usize))),
            watchers: Vec::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    pub fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Get a clone of the store for watchers
    pub fn get_store(&self) -> Arc<RwLock<EventStore<T>>> {
        self.store.clone()
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.read().unwrap()
    }

    /// List all data - returns all resources in the cache with resource version
    /// This is typically called by clients to get the full snapshot of data
    pub fn list(&self) -> ListData<T>
    where
        T: Clone,
    {
        self.list_owned()
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read().unwrap();
        let (data, resource_version) = store_guard.snapshot_owned();
        ListData::new(data, resource_version)
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Only needs the store to access data
    pub fn start_watcher_task(
        store: Arc<RwLock<EventStore<T>>>,
        notify: Arc<Notify>,
        watcher: WatchClient<T>,
    ) where
        T: Clone + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            let mut from_version = watcher.from_version;
            let sender = watcher.sender;
            let send_count = watcher.send_count;
            let last_send_time = watcher.last_send_time;

            loop {
                let result = {
                    let store_guard = store.read().unwrap();
                    store_guard.get_events_from_resource_version(from_version)
                };

                match result {
                    Ok((current_version, maybe_events)) => match maybe_events {
                        Some(events) if !events.is_empty() => {
                            let response = WatchResponse::new(events, current_version);

                            if sender.send(response).await.is_err() {
                                println!("CenterCache receiver dropped, client disconnected");
                                break;
                            }

                            from_version = current_version;

                            send_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if let Ok(mut last_time) = last_send_time.write() {
                                *last_time = Some(SystemTime::now());
                            }
                        }
                        _ => {
                            if current_version > from_version {
                                from_version = current_version;
                                continue;
                            } else {
                                notify.notified().await;
                            }
                        }
                    },
                    Err(err) => {
                        eprintln!(
                            "[CenterCache] watcher {} error fetching events: {}",
                            watcher.client_id, err
                        );

                        let response = WatchResponse::from_error(err.clone(), from_version);
                        let _ = sender.send(response).await;
                        break;
                    }
                }
            }
        });
    }

    /// Watch for changes starting from a specific version
    /// Returns a receiver that continuously receives WatchResponse updates
    ///
    /// This method will automatically start a watcher task that:
    /// 1. First checks if client is behind and sends initial data
    /// 2. Then loops waiting for notifications and sends updates
    pub fn watch(
        &mut self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<T>>
    where
        T: Clone + Send + Sync + 'static,
    {
        println!(
            "[CenterCache] new watch request client_id={} client_name={} from_version={} ready={} pending_watchers={}",
            client_id,
            client_name,
            from_version,
            *self.ready.read().unwrap(),
            self.watchers.len()
        );
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        // Get the shared notify and store
        let notify = self.get_notify();
        let store = self.get_store();

        let watcher = WatchClient {
            client_id,
            client_name,
            from_version,
            sender: data_tx,
            watch_start_time: SystemTime::now(),
            send_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_send_time: Arc::new(RwLock::new(None)),
        };
        self.watchers.push(watcher.clone());

        println!(
            "[CenterCache] watcher registered client_id={} total_watchers={}",
            watcher.client_id,
            self.watchers.len()
        );

        // Start the watcher task - only needs store
        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue
    fn push_event(&self, event_type: EventType, resource: T, resource_version: u64)
    where
        T: Clone + Send + 'static,
    {
        if !self.is_ready() {
            println!("trying push event when not ready!");
        }

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let store = self.store.clone();
                let notify = self.notify.clone();
                handle.spawn(async move {
                    {
                        let mut store_guard = store.write().unwrap();
                        store_guard.apply_event(event_type, resource, resource_version);
                    }
                    notify.notify_waiters();
                });
            }
            Err(_) => {
                {
                    let mut store_guard = self.store.write().unwrap();
                    store_guard.apply_event(event_type, resource, resource_version);
                }
                self.notify.notify_waiters();
            }
        }
    }
}

impl<T: Versionable + Resource + Clone + Send + Sync + 'static> EventDispatch<T> for ServerCache<T> {
    fn apply_change(&mut self, change: ResourceChange, resource: T)
    where
        T: Resource + Send + 'static,
    {
        let version = resource.get_version();
        if resource.get_version() == 0 {
            tracing::warn!(
                component = "cache_client",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = 0,
                "Applying change to cache with version 0"
            );
        } else {
            tracing::info!(
                component = "cache_server",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = resource.get_version(),
                "Applying change to cache"
            );
        }

        match change {
            ResourceChange::InitAdd => {
                let mut store_guard = self.store.write().unwrap();
                store_guard.init_add(version, resource);
            }
            ResourceChange::EventAdd => {
                self.push_event(EventType::Add, resource, version);
            }
            ResourceChange::EventUpdate => {
                self.push_event(EventType::Update, resource, version);
            }
            ResourceChange::EventDelete => {
                self.push_event(EventType::Delete, resource, version);
            }
        }
    }

    fn set_ready(&mut self) {
        *self.ready.write().unwrap() = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conf_sync::traits::ResourceChange;
    use crate::core::conf_sync::EventDispatch;
    use kube::api::{DynamicObject, ObjectMeta};
    use tokio::task::yield_now;
    use tokio::time::{sleep, Duration};

    #[derive(Debug, Clone)]
    struct TestResource {
        name: &'static str,
        version: u64,
        metadata: ObjectMeta,
    }

    impl PartialEq for TestResource {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name && self.version == other.version
        }
    }

    impl Eq for TestResource {}

    impl Versionable for TestResource {
        fn get_version(&self) -> u64 {
            self.version
        }
    }

    impl kube::Resource for TestResource {
        type DynamicType = ();
        type Scope = kube::core::ClusterResourceScope;

        fn kind(_: &Self::DynamicType) -> std::borrow::Cow<str> {
            "TestResource".into()
        }

        fn group(_: &Self::DynamicType) -> std::borrow::Cow<str> {
            "test.example.com".into()
        }

        fn version(_: &Self::DynamicType) -> std::borrow::Cow<str> {
            "v1".into()
        }

        fn plural(_: &Self::DynamicType) -> std::borrow::Cow<str> {
            "testresources".into()
        }

        fn meta(&self) -> &ObjectMeta {
            &self.metadata
        }

        fn meta_mut(&mut self) -> &mut ObjectMeta {
            &mut self.metadata
        }
    }

    async fn wait_for_async_store_update() {
        // Ensure spawned tasks have a chance to persist events into the store
        yield_now().await;
        sleep(Duration::from_millis(5)).await;
    }

    #[tokio::test]
    async fn event_add_stores_resource_and_updates_version() {
        let mut cache = ServerCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo",
            version: 1,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            resource.clone(),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], resource);
        assert_eq!(snapshot.resource_version, resource.version);
    }

    #[tokio::test]
    async fn event_update_replaces_existing_resource() {
        let mut cache = ServerCache::<TestResource>::new(10);
        let original = TestResource {
            name: "foo",
            version: 1,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };
        let updated = TestResource {
            name: "foo-updated",
            version: 1,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            original.clone(),
        );
        wait_for_async_store_update().await;

        cache.apply_change(
            ResourceChange::EventUpdate,
            updated.clone(),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], updated);
        assert_eq!(snapshot.resource_version, updated.version);
    }

    #[tokio::test]
    async fn event_delete_removes_resource() {
        let mut cache = ServerCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo",
            version: 42,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            resource.clone(),
        );
        wait_for_async_store_update().await;

        cache.apply_change(
            ResourceChange::EventDelete,
            resource.clone(),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert!(snapshot.data.is_empty());
        assert_eq!(snapshot.resource_version, resource.version);
    }
}
