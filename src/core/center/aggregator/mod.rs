pub struct ResourceAggregator;

impl ResourceAggregator {
    pub fn new() -> Self {
        Self
    }

    pub fn update_snapshot(
        &self,
        _controller_id: &str,
        _keys: Vec<crate::core::common::fed_sync::proto::ResourceKey>,
    ) {
    }
}
