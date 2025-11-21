use crate::core::conf_sync::GatewayBaseConf;
use async_trait::async_trait;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    /// Connect to the configuration source (e.g., etcd cluster, localpath)
    async fn connect(&self) -> anyhow::Result<()>;

    /// Load base configuration and return GatewayBaseConf
    /// This method should find GatewayClass, EdgionGatewayConfig, and Gateways,
    /// then assemble them into a GatewayBaseConf
    async fn load_base(&self) -> anyhow::Result<GatewayBaseConf>;

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> anyhow::Result<()>;

    /// Set ready state after initialization is complete
    async fn set_ready(&self);

    /// Main run loop for watching configuration changes
    async fn run(&self) -> anyhow::Result<()>;

    fn set_enable_resource_version_fix(&self);
}
