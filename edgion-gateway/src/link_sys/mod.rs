//! Gateway-side LinkSys runtime providers and logging sinks.
//!
//! Gateway-owned runtime providers handle Redis, Elasticsearch, Kafka, and Webhook
//! LinkSys resources. The Etcd provider and its runtime live in the shared
//! `edgion_common::link_sys_core` and are re-exported through this module.
//!
//! `HttpDns` and `CredentialSource` are valid `SystemConfig` variants, but neither
//! creates a generic Gateway runtime client: HttpDns is consumed by the DNS resolver
//! path, while CredentialSource is Controller-managed acquisition metadata.
//! LocalFile and OTLP are logging sinks, not LinkSys CRD provider variants.
//!
//! The `LinkSysStore` provides a unified store with `ConfHandler<LinkSys>` integration,
//! automatically dispatching resource changes to the appropriate sub-module managers.
//! Runtime clients are stored in separate ArcSwap stores for typed access.

pub mod providers;
pub mod runtime;
pub mod template;

// Re-exports of the LinkSys core (defined in `edgion_common::link_sys_core`) so
// gateway crate paths remain stable. New code should pick its home by the
// core/runtime boundary: backend-agnostic core logic (credential / TLS / health /
// lifecycle, or a provider the controller also needs) belongs in the common
// `link_sys_core`; data-plane runtime providers and stores belong in this module.
pub use edgion_common::link_sys_core::{credential, health, http_client, lifecycle, outbound_tls, tls, LinkSysHealth};

pub use providers::webhook::{get_webhook_call_handle, WebhookCallHandle};
pub use providers::{LocalFileWriter, LogType};
pub use runtime::{
    create_link_sys_handler, get_es_client, get_etcd_client, get_global_link_sys_store, get_kafka_client,
    get_redis_client, DataSender, LinkSysStore, LogMeta,
};
