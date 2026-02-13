//! Elasticsearch LinkSys runtime client.
//!
//! Provides an Elasticsearch client wrapper built from LinkSys CRD config
//! (`ElasticsearchClientConfig`), using `reqwest` as the underlying HTTP client.
//!
//! Zero new dependencies — reqwest, serde_json, base64, chrono are all already
//! in Cargo.toml. Compatible with Elasticsearch 7.x / 8.x and OpenSearch.
//!
//! Key components:
//! - `EsLinkClient` — core client wrapper with init/shutdown/health
//! - `EsBulkConfig` — resolved bulk ingest configuration
//! - `EsDataSender` — `DataSender<String>` impl for log shipping
//! - `ops` — high-level operations (cluster, index, document, search)
//!
//! # Usage
//!
//! ```ignore
//! use crate::core::link_sys::get_es_client;
//!
//! if let Some(client) = get_es_client("default/es-main") {
//!     // Single document
//!     let id = client.index_doc("my-index", &json!({"msg": "hello"})).await?;
//!
//!     // Bulk ingest (via background task)
//!     client.send_bulk(r#"{"msg": "log line"}"#.to_string()).await?;
//!
//!     // Search
//!     let result = client.search("my-index", &json!({"query": {"match_all": {}}})).await?;
//! }
//! ```

pub mod bulk;
pub mod client;
pub mod config_mapping;
pub mod data_sender;
pub mod ops;

// Re-export key types for convenient access
pub use client::EsLinkClient;
pub use config_mapping::EsBulkConfig;
pub use data_sender::EsDataSender;
pub use ops::{EsSearchHit, EsSearchResult, LinkSysHealth};
