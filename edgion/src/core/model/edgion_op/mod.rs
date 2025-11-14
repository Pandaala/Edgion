use crate::core::conf_sync::config_server::ConfigServer;
use std::sync::Arc;

pub mod admin;

const CACHE_CAPACITY: u32 = 1024;
pub const DEFAULT_GATEWAY_CLASS_KEY: &str = "default";

pub struct EdgionOpServer {
    config_server: Arc<ConfigServer>,
}

impl EdgionOpServer {
    pub fn new() -> Self {
        let config_server = Arc::new(ConfigServer::new());
        Self { config_server }
    }

    pub fn config_server(&self) -> Arc<ConfigServer> {
        self.config_server.clone()
    }

    pub fn run_op_server() {}

    pub fn run_admin_server() {}

    pub async fn shutdown(&mut self) {}
}
