use crate::core::conf_sync::GatewayBaseConf;
use crate::core::conf_sync::traits::ConfigServerEventDispatcher;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    /// Register a dispatcher for handling configuration events
    async fn register_dispatcher(&self, dispatcher: Arc<dyn ConfigServerEventDispatcher>);

    /// Connect to the configuration source (e.g., etcd cluster, localpath)
    async fn connect(&self) -> anyhow::Result<()>;

    /// Load base configuration and return GatewayBaseConf
    /// This method should find GatewayClass (by name), EdgionGatewayConfig (linked via parameters_ref),
    /// and Gateways (that reference the GatewayClass), then assemble them into a GatewayBaseConf
    async fn load_base(&self, gateway_class_name: &str) -> anyhow::Result<GatewayBaseConf>;

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> anyhow::Result<()>;

    /// Set ready state after initialization is complete
    async fn set_ready(&self);

    /// Main run loop for watching configuration changes
    async fn run(&self) -> anyhow::Result<()>;

    async fn set_enable_resource_version_fix(&self);
}
