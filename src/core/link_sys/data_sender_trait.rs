//! DataSender trait for external system integration

use async_trait::async_trait;
use anyhow::Result;

/// Trait for sending data to external systems (ES/Kafka/ClickHouse/etc.)
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

