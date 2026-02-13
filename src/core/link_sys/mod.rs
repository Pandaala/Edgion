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

mod conf_handler_impl;
mod data_sender_trait;
pub mod etcd;
pub mod link_sys_store;
pub mod local_file;
pub mod redis;
pub mod webhook;

pub use conf_handler_impl::create_link_sys_handler;
pub use data_sender_trait::DataSender;
pub use link_sys_store::{get_etcd_client, get_global_link_sys_store, get_redis_client, LinkSysStore};
pub use local_file::{LocalFileWriter, LogType};
