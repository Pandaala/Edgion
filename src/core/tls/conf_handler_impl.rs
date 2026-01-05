use super::tls_store::{get_global_tls_store, TlsStore};
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::EdgionTls;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler for Arc<TlsStore>
impl ConfHandler<EdgionTls> for Arc<TlsStore> {
    fn full_set(&self, data: &HashMap<String, EdgionTls>) {
        (**self).full_set(data.clone())
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionTls>,
        update: HashMap<String, EdgionTls>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, &remove)
    }
}

/// Implement ConfHandler directly for TlsStore
impl ConfHandler<EdgionTls> for TlsStore {
    fn full_set(&self, data: &HashMap<String, EdgionTls>) {
        self.full_set(data.clone())
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionTls>,
        update: HashMap<String, EdgionTls>,
        remove: HashSet<String>,
    ) {
        self.partial_update(add, update, &remove)
    }
}

/// Create a TlsStore handler for registration with ConfigClient
pub fn create_tls_handler() -> Box<dyn ConfHandler<EdgionTls> + Send + Sync> {
    Box::new(get_global_tls_store())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_tls::EdgionTlsSpec;
    use crate::types::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;

    fn create_test_tls(namespace: &str, name: &str, hosts: Vec<&str>) -> EdgionTls {
        let secret = Some(Secret {
            metadata: ObjectMeta {
                name: Some(format!("{}-secret", name)),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some({
                let mut map = std::collections::BTreeMap::new();
                map.insert("tls.crt".to_string(), ByteString(b"fake-cert".to_vec()));
                map.insert("tls.key".to_string(), ByteString(b"fake-key".to_vec()));
                map
            }),
            ..Default::default()
        });

        EdgionTls {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: hosts.iter().map(|h| h.to_string()).collect(),
                secret_ref: SecretObjectReference {
                    name: format!("{}-secret", name),
                    namespace: Some(namespace.to_string()),
                    group: None,
                    kind: None,
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret,
            },
            status: None,
        }
    }
}
