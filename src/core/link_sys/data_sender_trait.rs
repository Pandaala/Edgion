//! DataSender trait for external system integration
//!
//! Implementations:
//! - [x] LocalFileWriter - file-based with rotation
//! - [ ] EsSender - Elasticsearch with FailedCache support
//! - [ ] KafkaSender - Kafka async producer
//! - [ ] ClickHouseSender - ClickHouse batch insert

use async_trait::async_trait;
use anyhow::Result;

/// Trait for sending data to external systems
///
/// # FailedCache Pattern (for ES/remote senders)
/// ```ignore
/// struct EsSender {
///     client: EsClient,
///     failed_cache: Option<Box<dyn DataSender>>, // e.g., LocalFileWriter or Redis
/// }
/// ```
/// When ES is unavailable, logs are cached locally and replayed on recovery.
#[async_trait]
pub trait DataSender: Send + Sync {
    /// Initialize the sender connection
    async fn init(&mut self) -> Result<()>;
    
    /// Check if the sender is healthy
    fn healthy(&self) -> bool;
    
    /// Send data to the external system (takes ownership to avoid copy)
    async fn send(&self, data: String) -> Result<()>;
    
    /// Get the name of this sender (for logging)
    fn name(&self) -> &str;
}

