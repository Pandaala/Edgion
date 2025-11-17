use std::sync::Arc;

use anyhow::Result;
use clap::{Args, ValueEnum};
use std::path::PathBuf;
use crate::core::conf_sync::traits::{EventDispatcher};

pub mod etcd;
pub mod file_system;
pub mod traits;
pub use traits::ConfigLoader;

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

                let loader = FileSystemConfigLoader::new(path, dispatcher);
                Ok(Self { inner: loader })
            }
            LoaderKind::Etcd => Err(anyhow::anyhow!("etcd loader is not currently supported")),
        }
    }

    pub async fn run(self) -> Result<()> {

        tracing::info!("====> start connect...");
        // Connect to configuration source
        self.inner.connect().await?;

        tracing::info!("====> start bootstrap base conf...");
        // Bootstrap base configuration resources first
        self.inner.bootstrap_base_conf().await?;

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
