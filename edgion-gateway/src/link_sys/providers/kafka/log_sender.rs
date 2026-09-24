use anyhow::{anyhow, Result};
use async_trait::async_trait;

use edgion_resources::output::LinkSysRef;

use crate::link_sys::{get_kafka_client, DataSender};

pub struct KafkaLogSender {
    topic: String,
    key: String,
}

impl KafkaLogSender {
    pub fn new(link_sys_ref: LinkSysRef, topic: String) -> Self {
        Self {
            key: link_sys_ref.key(),
            topic,
        }
    }
}

#[async_trait]
impl DataSender<String> for KafkaLogSender {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }

    fn healthy(&self) -> bool {
        get_kafka_client(&self.key).map(|c| c.healthy()).unwrap_or(false)
    }

    async fn send(&self, data: String) -> Result<()> {
        match get_kafka_client(&self.key) {
            Some(client) => client.try_produce(&self.topic, data),
            None => Err(anyhow!("Kafka client '{}' not found in runtime", self.key)),
        }
    }

    fn name(&self) -> &str {
        &self.key
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/kafka/log_sender_test.rs"]
mod tests;
