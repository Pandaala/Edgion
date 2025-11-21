use crate::types::ResourceKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResourceChange {
    InitAdd,
    EventAdd,
    EventUpdate,
    EventDelete,
}

pub trait ConfigServerEventDispatcher: Send + Sync {
    fn apply_resource_change(&self, change: ResourceChange, resource_type: Option<ResourceKind>, data: String);

    fn enable_version_fix_mode(&self);

    fn set_ready(&self);
}

pub trait ConfigClientEventDispatcher: Send + Sync {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    );
}
