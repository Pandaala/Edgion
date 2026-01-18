//! Controller context shared by all reconcilers

use std::sync::Arc;

use super::status::StatusStore;
use crate::core::conf_sync::conf_server::ConfigServer;

/// Controller context shared by all reconcilers
#[derive(Clone)]
pub struct ControllerContext {
    pub config_server: Arc<ConfigServer>,
    pub status_store: Arc<dyn StatusStore>,
    pub gateway_class_name: String,
}

impl std::fmt::Debug for ControllerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerContext")
            .field("gateway_class_name", &self.gateway_class_name)
            .finish_non_exhaustive()
    }
}
