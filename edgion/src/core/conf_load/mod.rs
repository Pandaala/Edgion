use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::conf_sync::traits::EventDispatcher;

pub mod etcd;
pub mod file_system;

pub type SharedDispatcher = Arc<Mutex<Box<dyn EventDispatcher>>>;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    async fn run(self: Arc<Self>) -> anyhow::Result<()>;
}

pub use etcd::EtcdConfigLoader;
pub use file_system::FileSystemConfigLoader;
