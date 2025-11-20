use crate::types::ResourceKind;
use async_trait::async_trait;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    /// Connect to the configuration source (e.g., etcd cluster, localpath)
    async fn connect(&self) -> anyhow::Result<()>;

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    /// If kind is specified, only load resources of that kind
    async fn bootstrap_base_conf(&self, kind: Option<ResourceKind>) -> anyhow::Result<()>;

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> anyhow::Result<()>;

    /// Set ready state after initialization is complete
    async fn set_ready(&self);

    /// Main run loop for watching configuration changes
    async fn run(&self) -> anyhow::Result<()>;

    fn set_enable_resource_version_fix(&self);
}
