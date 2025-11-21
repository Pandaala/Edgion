use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::SystemTime;
use crate::core::gateway::server_header::ServerHeaderOpts;

pub struct EdgionHttp {
    pub gateway_class_name: String,
    pub gateway_namespace: String,
    pub gateway_name: String,

    pub server_start_time: SystemTime,

    pub server_header_opts: ServerHeaderOpts,

    // counter
    pub ctx_cnt: Arc<AtomicUsize>,
}

