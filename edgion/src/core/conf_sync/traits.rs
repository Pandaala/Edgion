use crate::types::ResourceKind;

#[derive(Clone, Copy, Debug)]
pub enum ResourceChange {
    InitAdd,
    EventAdd,
    EventUpdate,
    EventDelete,
}

pub trait EventDispatcher: Send + Sync {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    );

    /// Apply base configuration change (GatewayClass, EdgionGatewayConfig, Gateway)
    /// This method is used during initialization to populate base_conf
    fn apply_base_conf(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    );

    fn set_ready(&self);
}
