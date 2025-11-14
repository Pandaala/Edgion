use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use clap::{Args, ValueEnum};
use std::path::PathBuf;

use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;

pub mod etcd;
pub mod file_system;

pub type SharedDispatcher = Arc<tokio::sync::Mutex<ConfigServerDispatcher>>;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    async fn run(self: Arc<Self>) -> anyhow::Result<()>;
}

pub use etcd::EtcdConfigLoader;
pub use file_system::FileSystemConfigLoader;

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum LoaderKind {
    Filesystem,
    Etcd,
}

#[derive(Args, Debug, Clone)]
pub struct LoaderArgs {
    /// Configuration loader type (currently only filesystem is supported)
    #[arg(long, value_enum, value_name = "TYPE", default_value = "filesystem")]
    pub loader: LoaderKind,

    /// Root directory for filesystem loader
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,

    /// Etcd node addresses (not currently supported)
    #[arg(long = "etcd-endpoint", value_name = "URL")]
    pub etcd_endpoint: Vec<String>,

    /// Etcd key prefix (not currently supported)
    #[arg(long = "etcd-prefix", value_name = "PREFIX")]
    pub etcd_prefix: Option<String>,
}

pub struct Loader {
    inner: Arc<dyn ConfigLoader>,
}

impl Loader {
    pub fn from_args(args: &LoaderArgs, dispatcher: Arc<dyn EventDispatcher>) -> Result<Self> {
        match args.loader {
            LoaderKind::Filesystem => {
                const DEFAULT_FILESYSTEM_DIR: &str = "edgion/config/examples";
                let dir = args
                    .dir
                    .clone()
                    .unwrap_or_else(|| DEFAULT_FILESYSTEM_DIR.to_string());
                let path = PathBuf::from(&dir);
                if !path.exists() {
                    return Err(anyhow::anyhow!(
                        "configuration directory {:?} does not exist",
                        path
                    ));
                }

                let loader = FileSystemConfigLoader::new(path, dispatcher, None);
                Ok(Self { inner: loader })
            }
            LoaderKind::Etcd => Err(anyhow::anyhow!("etcd loader is not currently supported")),
        }
    }

    pub async fn run(self) -> Result<()> {
        self.inner.run().await
    }
}

struct ConfigServerDispatcher {
    server: Arc<ConfigServer>,
}

impl EventDispatcher for ConfigServerDispatcher {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {
        self.server
            .apply_resource_change(change, resource_type, data, resource_version);
    }

    fn set_ready(&self) {
        self.server.set_ready();
    }
}
