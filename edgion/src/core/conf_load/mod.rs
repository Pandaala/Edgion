use std::sync::Arc;

use crate::core::conf_sync::traits::ConfigServerEventDispatcher;
use anyhow::Result;
use clap::{Args, ValueEnum};
use std::path::PathBuf;

pub mod etcd;
pub mod file_system_loader;
pub mod traits;
pub use traits::ConfigLoader;

pub use etcd::EtcdConfigLoader;
pub use file_system_loader::LocalPathLoader;

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum LoaderKind {
    NotSupport,
    LocalPath,
    Etcd,
}

#[derive(Args, Debug, Clone)]
pub struct LoaderArgs {
    /// Configuration loader type (currently only localpath is supported)
    #[arg(long, value_enum, value_name = "TYPE")]
    pub loader: LoaderKind,

    /// Root directory for localpath loader
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
    pub fn from_args(args: &LoaderArgs) -> Result<Self> {
        match args.loader {
            LoaderKind::LocalPath => {
                const DEFAULT_LOCAL_PATH_DIR: &str = "edgion/config/examples";
                let dir = args.dir.clone().unwrap_or_else(|| DEFAULT_LOCAL_PATH_DIR.to_string());
                let path = PathBuf::from(&dir);
                if !path.exists() {
                    return Err(anyhow::anyhow!("configuration directory {:?} does not exist", path));
                }

                let loader = LocalPathLoader::new(path);
                Ok(Self { inner: loader })
            }
            LoaderKind::Etcd => Err(anyhow::anyhow!("etcd loader is not currently supported")),
            LoaderKind::NotSupport => Err(anyhow::anyhow!("not support loader")),
        }
    }

    /// Register a dispatcher for handling configuration events
    pub async fn register_dispatcher(&self, dispatcher: Arc<dyn ConfigServerEventDispatcher>) {
        self.inner.register_dispatcher(dispatcher).await;
    }

    /// Load base configuration (GatewayClass, EdgionGatewayConfig, Gateway)
    pub async fn load_base(&self) -> Result<crate::core::conf_sync::GatewayBaseConf> {
        self.inner.load_base().await
    }

    pub async fn run(self) -> Result<()> {
        tracing::info!("====> start connect...");
        // Connect to configuration source
        self.inner.connect().await?;

        self.inner.set_enable_resource_version_fix().await;

        tracing::info!("====> start bootstrap user conf...");
        // Bootstrap user configuration resources
        self.inner.bootstrap_user_conf().await?;

        tracing::info!("====> Bootstrapped, set ready");
        // Set ready state
        self.inner.set_ready().await;

        tracing::info!("====> Loader running...");
        // Start watching for changes
        self.inner.run().await
    }
}
