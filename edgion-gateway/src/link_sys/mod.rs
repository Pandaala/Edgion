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

// Re-exports from `edgion_common::link` so gateway crate paths remain stable.
pub use edgion_common::link::{credential, health, lifecycle, tls, LinkSysHealth};

pub use providers::webhook::{
    get_webhook_call_handle, WebhookCallError, WebhookCallHandle, WebhookCallRequest, WebhookCallResult,
};
pub use providers::{LocalFileWriter, LogType};
pub use runtime::{
    create_link_sys_handler, get_es_client, get_etcd_client, get_global_link_sys_store, get_kafka_client,
    get_redis_client, DataSender, LinkSysStore, LogMeta,
};
