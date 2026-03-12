//! Redis-backed DataSender for failed log caching.
//!
//! When ES/Kafka is unavailable, logs are RPUSH'd to a Redis list.
//! A separate recovery process can LPOP and replay them later.
//!
//! Key format: `edgion:failed_cache:{sink_name}`
//! List is capped at `max_entries` to prevent unbounded growth.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::core::gateway::link_sys::DataSender;

use super::client::RedisLinkClient;

/// Redis-backed DataSender for failed log caching.
///
/// Stores log entries as a Redis list using RPUSH. When the list exceeds
/// `max_entries`, LTRIM is used to cap the list (oldest entries dropped).
pub struct RedisDataSender {
    client: Arc<RedisLinkClient>,
    list_key: String,
    max_entries: u64,
}

impl RedisDataSender {
    /// Create a new RedisDataSender.
    ///
    /// - `client`: A shared RedisLinkClient (already initialized).
    /// - `sink_name`: Used to derive the Redis list key.
    /// - `max_entries`: Maximum list length. Older entries are trimmed.
    pub fn new(client: Arc<RedisLinkClient>, sink_name: &str, max_entries: u64) -> Self {
        Self {
            client,
            list_key: format!("edgion:failed_cache:{}", sink_name),
            max_entries,
        }
    }

    /// Get the Redis list key used by this sender.
    pub fn list_key(&self) -> &str {
        &self.list_key
    }
}

#[async_trait]
impl DataSender<String> for RedisDataSender {
    async fn init(&mut self) -> Result<()> {
        // Client should already be initialized via RedisLinkClient::init()
        Ok(())
    }

    fn healthy(&self) -> bool {
        self.client.healthy()
    }

    async fn send(&self, data: String) -> Result<()> {
        // RPUSH to list
        let len = self.client.rpush(&self.list_key, vec![data]).await?;

        // Trim list if exceeds max_entries (LTRIM keeps [0, max_entries-1])
        if len > self.max_entries {
            self.client
                .ltrim(&self.list_key, 0, (self.max_entries as i64) - 1)
                .await?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "redis-failed-cache"
    }
}
