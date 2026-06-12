use super::tls_store::{get_global_tls_store, TlsStore};
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use edgion_resources::EdgionTls;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler directly for TlsStore
impl ConfHandler<EdgionTls> for TlsStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionTls>>) {
        self.replace_all_tls(arc_map_to_owned(data))
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionTls>>,
        update: HashMap<String, Arc<EdgionTls>>,
        remove: HashSet<String>,
    ) {
        let add_owned = arc_map_into_owned(add);
        let update_owned = arc_map_into_owned(update);
        self.apply_partial_update(add_owned, update_owned, &remove)
    }
}

/// Create a TlsStore handler for registration with ConfigClient
pub fn create_tls_handler() -> Box<dyn ConfHandler<EdgionTls> + Send + Sync> {
    Box::new(get_global_tls_store())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use edgion_resources::resources::edgion_tls::EdgionTlsSpec;
    use edgion_resources::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;

    #[allow(dead_code)]
    fn create_test_tls(namespace: &str, name: &str, hosts: Vec<&str>) -> EdgionTls {
        let secret = Some(Secret {
            metadata: ObjectMeta {
                name: Some(format!("{}-secret", name)),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some({
                let mut map = std::collections::BTreeMap::new();
                map.insert(CERT.to_string(), ByteString(b"fake-cert".to_vec()));
                map.insert(KEY.to_string(), ByteString(b"fake-key".to_vec()));
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
                extend: None,
                secret,
                resolved_listeners: None,
            },
            status: None,
        }
    }
}
