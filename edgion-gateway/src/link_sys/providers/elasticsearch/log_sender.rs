use anyhow::{anyhow, Result};
use async_trait::async_trait;

use edgion_resources::output::LinkSysRef;

use crate::link_sys::{get_es_client, DataSender};

pub struct EsLogSender {
    key: String,
}

impl EsLogSender {
    pub fn new(link_sys_ref: LinkSysRef) -> Self {
        Self {
            key: link_sys_ref.key(),
        }
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
mod tests {
    use super::*;
    use edgion_resources::output::LinkSysRef;

    #[test]
    fn test_es_log_sender_name_returns_key() {
        let r = LinkSysRef {
            namespace: "default".to_string(),
            name: "es-main".to_string(),
        };
        let sender = EsLogSender::new(r);
        assert_eq!(sender.name(), "default/es-main");
    }

    #[tokio::test]
    async fn test_es_log_sender_healthy_false_when_not_registered() {
        let r = LinkSysRef {
            namespace: "default".to_string(),
            name: "nonexistent".to_string(),
        };
        let sender = EsLogSender::new(r);
        assert!(!sender.healthy());
    }
}
