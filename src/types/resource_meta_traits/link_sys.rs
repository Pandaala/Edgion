//! ResourceMeta implementation for LinkSys

use crate::types::resource_kind::ResourceKind;
use crate::types::resources::LinkSys;

use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for LinkSys {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::LinkSys
    }
    
    fn kind_name() -> &'static str {
        "LinkSys"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn pre_parse(&mut self) {
        // Validate configuration based on system type
        match self.spec.sys_type {
            crate::types::resources::SystemType::Redis => {
                if let Some(redis_config) = &self.spec.redis {
                    // Validate endpoints
                    if redis_config.endpoints.is_empty() {
                        tracing::warn!(
                            "LinkSys {}: Redis configuration has no endpoints",
                            self.key_name()
                        );
                    }
                    
                    // Validate topology consistency
                    if let Some(topology) = &redis_config.topology {
                        match topology.mode {
                            crate::types::resources::RedisTopologyMode::Sentinel => {
                                if topology.sentinel.is_none() {
                                    tracing::warn!(
                                        "LinkSys {}: Sentinel mode specified but sentinel config is missing",
                                        self.key_name()
                                    );
                                }
                            }
                            crate::types::resources::RedisTopologyMode::Cluster => {
                                if topology.cluster.is_none() {
                                    tracing::warn!(
                                        "LinkSys {}: Cluster mode specified but cluster config is missing",
                                        self.key_name()
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    tracing::warn!(
                        "LinkSys {}: System type is Redis but redis config is missing",
                        self.key_name()
                    );
                }
            }
            _ => {
                tracing::warn!(
                    "LinkSys {}: System type {:?} is not yet implemented",
                    self.key_name(),
                    self.spec.sys_type
                );
            }
        }
    }
}

