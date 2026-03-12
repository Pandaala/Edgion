//! Link external systems (ES/Kafka/ClickHouse/Redis/Etcd/Webhook/etc.)
//!
//! This module manages connections to external systems defined by LinkSys CRD resources.
//! Each system type has its own sub-module for runtime management:
//! - `redis/` — Redis client (standalone/sentinel/cluster) via fred
//! - `etcd/` — Etcd client (v3 API) via etcd-client
//! - `webhook/` — HTTP webhook service for KeyGet::Webhook resolution
//! - `local_file/` — File-based log writer with rotation
//!
//! The `LinkSysStore` provides a unified store with `ConfHandler<LinkSys>` integration,
//! automatically dispatching resource changes to the appropriate sub-module managers.
//! Runtime clients are stored in separate ArcSwap stores for typed access.

pub mod providers;
pub mod runtime;

pub use providers::{LocalFileWriter, LogType};
pub use runtime::{
    create_link_sys_handler, get_es_client, get_etcd_client, get_global_link_sys_store, get_redis_client, DataSender,
    LinkSysStore,
};
