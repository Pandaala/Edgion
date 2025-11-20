use std::sync::Arc;

use anyhow::Result;
use clap::{Args, ValueEnum};
use std::path::PathBuf;
use crate::core::conf_sync::traits::{ConfigServerEventDispatcher};
use crate::types::ResourceKind;

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
    pub fn from_args(args: &LoaderArgs, dispatcher: Arc<dyn ConfigServerEventDispatcher>) -> Result<Self> {
        match args.loader {
            LoaderKind::LocalPath => {
                const DEFAULT_LOCAL_PATH_DIR: &str = "edgion/config/examples";
                let dir = args
                    .dir
                    .clone()
                    .unwrap_or_else(|| DEFAULT_LOCAL_PATH_DIR.to_string());
                let path = PathBuf::from(&dir);
                if !path.exists() {
                    return Err(anyhow::anyhow!(
                        "configuration directory {:?} does not exist",
                        path
                    ));
                }

                let loader = LocalPathLoader::new(path, dispatcher);
                Ok(Self { inner: loader })
            }
            LoaderKind::Etcd => Err(anyhow::anyhow!("etcd loader is not currently supported")),
            LoaderKind::NotSupport => Err(anyhow::anyhow!("not support loader")),
        }
    }

    pub async fn run(self) -> Result<()> {

        tracing::info!("====> start connect...");
        // Connect to configuration source
        self.inner.connect().await?;

        tracing::info!("====> start bootstrap base conf...");
        // Bootstrap base configuration resources in order:
        // 1. GatewayClass (must be loaded first)
        tracing::info!("====> loading GatewayClass...");
        self.inner.bootstrap_base_conf(Some(ResourceKind::GatewayClass)).await?;
        
        // 2. EdgionGatewayConfig (referenced by GatewayClass)
        tracing::info!("====> loading EdgionGatewayConfig...");
        self.inner.bootstrap_base_conf(Some(ResourceKind::EdgionGatewayConfig)).await?;
        
        // 3. Gateway (uses GatewayClass)
        tracing::info!("====> loading Gateway...");
        self.inner.bootstrap_base_conf(Some(ResourceKind::Gateway)).await?;

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
