//! ConfHandler implementation for LinkSys resources.
//!
//! Integrates LinkSysStore with the configuration sync system so that
//! LinkSys resource changes are automatically dispatched to the appropriate
//! sub-module managers (Webhook, Redis, Etcd, etc.).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::store::{get_global_link_sys_store, LinkSysStore};
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use edgion_resources::resources::link_sys::LinkSys;

/// Create a LinkSysStore handler for registration with ConfigClient
pub fn create_link_sys_handler() -> Box<dyn ConfHandler<LinkSys> + Send + Sync> {
    Box::new(get_global_link_sys_store())
}

impl ConfHandler<LinkSys> for LinkSysStore {
    fn full_set(&self, data: &HashMap<String, Arc<LinkSys>>) {
        tracing::info!(component = "link_sys_store", cnt = data.len(), "full set");

        self.replace_all(arc_map_to_owned(data));
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<LinkSys>>,
        update: HashMap<String, Arc<LinkSys>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "link_sys_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut add_or_update = arc_map_into_owned(add);
        add_or_update.extend(arc_map_into_owned(update));

        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys/runtime/conf_handler_test.rs"]
mod tests;
