use async_trait::async_trait;

#[async_trait]
pub trait ConfigLoader: Send + Sync {
    /// Connect to the configuration source (e.g., etcd cluster, filesystem)
    async fn connect(&self) -> anyhow::Result<()>;

    /// Bootstrap and load all existing configurations
    async fn bootstrap_existing(&self) -> anyhow::Result<()>;

    /// Set ready state after initialization is complete
    async fn set_ready(&self);

    /// Main run loop for watching configuration changes
    async fn run(&self) -> anyhow::Result<()>;
}
