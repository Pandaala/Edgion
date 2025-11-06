use std::collections::HashMap;

pub struct HubCache<T> {
    // data
    data: HashMap<String, T>,

    // version,
    resource_version: u64,
}