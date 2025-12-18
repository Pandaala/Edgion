use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::EdgionTls;
use super::tls_store::{TlsStore, get_global_tls_store};

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
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;
    use crate::types::resources::edgion_tls::EdgionTlsSpec;
    use crate::types::resources::gateway::SecretObjectReference;

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
                secret,
            },
            status: None,
        }
    }

    #[test]
    fn test_full_set_via_handler() {
        let handler = create_tls_handler();
        
        let mut data = HashMap::new();
        data.insert(
            "default/tls1".to_string(),
            create_test_tls("default", "tls1", vec!["example.com"]),
        );
        
        handler.full_set(&data);
        
        let store = get_global_tls_store();
        assert!(store.contains("default/tls1"));
    }

    #[test]
    fn test_partial_update_via_handler() {
        let handler = create_tls_handler();
        
        // Add
        let mut add = HashMap::new();
        add.insert(
            "default/tls1".to_string(),
            create_test_tls("default", "tls1", vec!["example.com"]),
        );
        
        handler.partial_update(add, HashMap::new(), HashSet::new());
        
        let store = get_global_tls_store();
        assert!(store.contains("default/tls1"));
        
        // Remove
        let mut remove = HashSet::new();
        remove.insert("default/tls1".to_string());
        
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert!(!store.contains("default/tls1"));
    }
}

