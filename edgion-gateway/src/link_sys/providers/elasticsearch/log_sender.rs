use anyhow::{anyhow, Result};
use async_trait::async_trait;

use edgion_resources::output::LinkSysRef;

use crate::link_sys::{get_es_client, DataSender};

pub struct EsLogSender {
    key: String,
}

impl EsLogSender {
    pub fn new(link_sys_ref: LinkSysRef) -> Result<Self> {
        Ok(Self {
            key: link_sys_ref.key().map_err(anyhow::Error::msg)?,
        })
    }
}

#[async_trait]
impl DataSender<String> for EsLogSender {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }

    fn healthy(&self) -> bool {
        get_es_client(&self.key).map(|c| c.healthy()).unwrap_or(false)
    }

    async fn send(&self, data: String) -> Result<()> {
        match get_es_client(&self.key) {
            Some(client) => client.try_send_bulk(data),
            None => Err(anyhow!("ES client '{}' not found in runtime", self.key)),
        }
    }

    fn name(&self) -> &str {
        &self.key
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/elasticsearch/log_sender_test.rs"]
mod tests;
