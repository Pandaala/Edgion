//! Elasticsearch LinkSys runtime client.
//!
//! Provides an Elasticsearch client wrapper built from LinkSys CRD config
//! (`ElasticsearchClientConfig`), using `reqwest` as the underlying HTTP client.
//!
//! Compatible with Elasticsearch 7.x / 8.x and OpenSearch.
//!
//! Key components:
//! - `EsLinkClient` — core client wrapper with init/shutdown/health
//! - `EsBulkConfig` — resolved bulk ingest configuration
//! - `ops` — high-level operations (cluster, index, document, search)
//!
//! # Usage
//!
//! ```ignore
//! use crate::link_sys::get_es_client;
//!
//! if let Some(client) = get_es_client("default/es-main") {
//!     // Single document
//!     let id = client.index_doc("my-index", &json!({"msg": "hello"})).await?;
//!
//!     // Bulk ingest (via background task)
//!     client.try_send_bulk(r#"{"msg": "log line"}"#.to_string())?; // non-blocking; use send_bulk() if backpressure is needed
//!
//!     // Search
//!     let result = client.search("my-index", &json!({"query": {"match_all": {}}})).await?;
//! }
//! ```

pub(crate) mod apply;
pub mod bulk;
pub mod client;
pub mod config_mapping;
pub mod log_sender;
pub mod ops;

pub(crate) use apply::EsApply;
pub use client::EsLinkClient;
pub use config_mapping::EsBulkConfig;
pub use log_sender::EsLogSender;
pub use ops::{EsSearchHit, EsSearchResult};
