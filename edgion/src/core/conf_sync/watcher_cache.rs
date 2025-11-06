use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, Notify};

/// List data response structure
#[derive(Debug, Clone)]
pub struct ListData<T> {
    pub data: Vec<T>,
    pub resource_version: u64,
}

impl<T> ListData<T> {
    pub fn new(data: Vec<T>, resource_version: u64) -> Self {
        Self {
            data,
            resource_version,
        }
    }
}

/// Watch response structure containing events and current version
#[derive(Debug, Clone)]
pub struct WatchResponse<T> {
    pub events: Vec<WatcherEvent<T>>,
    pub resource_version: u64,
}

impl<T> WatchResponse<T> {
    pub fn new(events: Vec<WatcherEvent<T>>, resource_version: u64) -> Self {
        Self {
            events,
            resource_version,
        }
    }
}

/// Pending watch request waiting for notification
#[derive(Clone)]
pub struct PendingWatch<T> {
    pub client_id: String,
    pub client_name: String,
    pub from_version: u64,
    pub sender: mpsc::Sender<WatchResponse<T>>, // Bounded for data
    pub watch_start_time: SystemTime,           // Watch 开始时间
    pub send_count: Arc<std::sync::atomic::AtomicU64>, // 发送计数（使用 Arc 以便在协程中更新）
    pub last_send_time: Arc<std::sync::RwLock<Option<SystemTime>>>, // 上次发送时间（使用 Arc 以便在协程中更新）
}

/// Trait for resources that have a version
pub trait Versionable {
    /// Get the resource version
    fn get_version(&self) -> u64;
}

/// Trait for handling resource events
pub trait EventDispatch<T> {
    /// Initialize by adding a resource
    fn init_add(&mut self, resource: T);

    /// Set the dispatcher as ready
    fn set_ready(&mut self);

    /// Handle add event
    fn event_add(&mut self, resource: T);

    /// Handle update event
    fn event_update(&mut self, resource: T);

    /// Handle delete event
    fn event_del(&mut self, resource: T);
}

#[derive(Debug, Clone)]
pub enum EventType {
    Update,
    Delete,
    Add,
}

#[derive(Debug, Clone)]
pub struct WatcherEvent<T> {
    pub event_type: EventType,
    pub resource_version: u64,
    pub data: T,
}

/// 事件存储 - 循环队列
pub struct CacheStore<T> {
    capacity: u32,
    cache: Vec<WatcherEvent<T>>,
    start_index: u32,
    end_index: u32,
    resource_version: u64,
}

impl<T: Clone> CacheStore<T> {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            cache: Vec::with_capacity(capacity as usize),
            start_index: 0,
            end_index: 0,
            resource_version: 0,
        }
    }

    /// 更新：添加新事件到循环队列
    pub fn mut_update(&mut self, event: WatcherEvent<T>) {
        self.resource_version = event.resource_version;

        if (self.cache.len() as u32) < self.capacity {
            self.cache.push(event);
            self.end_index = self.cache.len() as u32;
        } else {
            let index = (self.end_index % self.capacity) as usize;
            self.cache[index] = event;
            self.end_index = self.end_index.wrapping_add(1);

            if self.end_index.wrapping_sub(self.start_index) > self.capacity {
                self.start_index = self.end_index.wrapping_sub(self.capacity);
            }
        }
    }

    /// 查询：获取从指定版本开始的事件
    pub fn get_events_from_resource_version(
        &self,
        from_version: u64,
    ) -> (Vec<WatcherEvent<T>>, u64) {
        let mut events = Vec::new();

        let count = if (self.cache.len() as u32) < self.capacity {
            self.cache.len() as u32
        } else {
            self.capacity
        };

        for i in 0..count {
            let index = ((self.start_index + i) % self.capacity) as usize;
            if index < self.cache.len() {
                let event = &self.cache[index];
                if event.resource_version > from_version {
                    events.push(event.clone());
                }
            }
        }

        (events, self.resource_version)
    }
}

pub struct WatcherCache<T> {
    // data
    data: HashMap<String, T>,
    ready: bool,

    // 使用 CacheStore 替代原来的字段
    store: Arc<tokio::sync::RwLock<CacheStore<T>>>,

    // pending watch requests
    watchers: Vec<PendingWatch<T>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: Versionable> WatcherCache<T> {
    pub fn new(capacity: u32) -> Self
    where
        T: Clone,
    {
        Self {
            data: HashMap::new(),
            ready: false,
            store: Arc::new(tokio::sync::RwLock::new(CacheStore::new(capacity))),
            watchers: Vec::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    pub fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Get a clone of the store for watchers
    pub fn get_store(&self) -> Arc<tokio::sync::RwLock<CacheStore<T>>> {
        self.store.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// List all data - returns all resources in the cache with resource version
    /// This is typically called by clients to get the full snapshot of data
    pub fn list(&self) -> ListData<&T> {
        let data: Vec<&T> = self.data.values().collect();
        let resource_version = self.store.blocking_read().resource_version;
        ListData::new(data, resource_version)
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let data: Vec<T> = self.data.values().cloned().collect();
        let resource_version = self.store.blocking_read().resource_version;
        ListData::new(data, resource_version)
    }

    /// Notify all pending watchers (non-blocking)
    /// Uses shared Notify to broadcast to all watchers at once
    fn notify_watchers(&mut self) {
        // Notify all waiting tasks at once - much more efficient!
        self.notify.notify_waiters();
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Only needs the store to access data
    pub fn start_watcher_task(
        store: Arc<tokio::sync::RwLock<CacheStore<T>>>,
        notify: Arc<Notify>,
        watcher: PendingWatch<T>,
    ) where
        T: Clone + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            let mut from_version = watcher.from_version;
            let sender = watcher.sender;
            let send_count = watcher.send_count;
            let last_send_time = watcher.last_send_time;

            loop {
                // 简单的调用
                let (events, current_version) = {
                    let store_guard = store.read().await;
                    store_guard.get_events_from_resource_version(from_version)
                };

                if !events.is_empty() {
                    from_version = current_version;
                    let response = WatchResponse::new(events, current_version);

                    if sender.send(response).await.is_err() {
                        // Client disconnected, exit loop
                        break;
                    }

                    // 更新发送计数和时间
                    send_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut last_time) = last_send_time.write() {
                        *last_time = Some(SystemTime::now());
                    }
                }

                // Wait for next notification
                notify.notified().await;
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
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        // Get the shared notify and store
        let notify = self.get_notify();
        let store = self.get_store();

        let watcher = PendingWatch {
            client_id,
            client_name,
            from_version,
            sender: data_tx,
            watch_start_time: SystemTime::now(),
            send_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_send_time: Arc::new(std::sync::RwLock::new(None)),
        };
        self.watchers.push(watcher.clone());

        // Start the watcher task - only needs store
        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue
    fn add_event(&mut self, event_type: EventType, resource: T)
    where
        T: Clone,
    {
        let version = resource.get_version();

        let event = WatcherEvent {
            event_type,
            resource_version: version,
            data: resource,
        };

        // 使用 mut_update
        let mut store_guard = self.store.blocking_write();
        store_guard.mut_update(event);
        drop(store_guard);

        // Notify all pending watchers (non-blocking)
        if !self.watchers.is_empty() {
            self.notify_watchers();
        }
    }
}

impl<T: Versionable + Clone> EventDispatch<T> for WatcherCache<T> {
    fn init_add(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource);
    }

    fn set_ready(&mut self) {
        self.ready = true;
    }

    fn event_add(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource.clone());
        self.add_event(EventType::Add, resource);
    }

    fn event_update(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource.clone());
        self.add_event(EventType::Update, resource);
    }

    fn event_del(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.remove(&version.to_string());
        self.add_event(EventType::Delete, resource);
    }
}
