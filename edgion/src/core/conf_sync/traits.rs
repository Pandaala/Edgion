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
        &mut self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    );

    fn set_ready(&mut self);
}
