use crate::core::host_match::HashHost;
use crate::types::err::EdError;
use crate::types::EdgionTls;
use anyhow::Result;
use k8s_openapi::api::core::v1::Secret;
use parking_lot::RwLock;
use std::collections::{HashMap, LinkedList};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct TlsWithSecret {
    pub tls: Arc<EdgionTls>,
    pub secret: Arc<Secret>,
}

impl TlsWithSecret {
    pub fn new(tls: Arc<EdgionTls>, secret: Arc<Secret>) -> Self {
        Self { tls, secret }
    }

    pub fn cert_pem(&self) -> Result<String> {
        let data = self
            .secret
            .data
            .as_ref()
            .ok_or_else(|| (anyhow::anyhow!("Secret data not found")))?;
        let cert_pem = data
            .get("tls.crt")
            .ok_or_else(|| (anyhow::anyhow!("Secret data tls.crt not found")))?;
        String::from_utf8(cert_pem.0.clone()).map_err(|e| anyhow::anyhow!(e))
    }

    pub fn key_pen(&self) -> Result<String> {
        let data = self
            .secret
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret data not found"))?;
        let key_pen = data
            .get("tls.key")
            .ok_or_else(|| anyhow::anyhow!("Secret data tls.key not found"))?;
        String::from_utf8(key_pen.0.clone()).map_err(|e| anyhow::anyhow!(e))
    }
}

pub struct TlsCertMatcher {
    matcher: Arc<RwLock<HashHost<LinkedList<Arc<TlsWithSecret>>>>>,
    tls_secret_map: HashMap<String, TlsWithSecret>,
}

impl TlsCertMatcher {
    pub fn new() -> Self {
        Self {
            matcher: Arc::new(RwLock::new(HashHost::new())),
            tls_secret_map: HashMap::new(),
        }
    }

    pub fn add(&self, tls: Arc<TlsWithSecret>) -> Result<()> {
        let mut matcher = self.matcher.write();

        for host in tls.tls.spec.hosts.iter() {
            // Try to get a mutable reference
            if let Some(tls_list) = matcher.get_mut(host) {
                // If exists, add to the front of the list
                tls_list.push_front(tls.clone());
            } else {
                // If not exists, create new list and insert
                let mut tls_list = LinkedList::new();
                tls_list.push_front(tls.clone());
                matcher.insert(host, tls_list);
            }
        }

        Ok(())
    }

    pub fn match_sni(&self, sni: &str) -> Result<Arc<TlsWithSecret>, EdError> {
        let tls_list = self.matcher.read().get(sni).cloned().unwrap_or_default();
        if tls_list.is_empty() {
            return Err(EdError::SniNotMatch(sni.to_string()));
        }
        if let Some(t) = tls_list.front() {
            Ok(t.clone())
        } else {
            Err(EdError::SniNotMatch(sni.to_string()))
        }
    }
}
