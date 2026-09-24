use super::*;
use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

fn create_test_secret(namespace: &str, name: &str, cert: &str, key: &str) -> Secret {
    let mut data = std::collections::BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(cert.as_bytes().to_vec()));
    data.insert(KEY.to_string(), ByteString(key.as_bytes().to_vec()));

    Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    }
}

#[test]
fn test_secret_store_basic() {
    let store = SecretStore::new();

    assert!(store.get(Some("default"), "my-secret").is_none());

    let secret = create_test_secret("default", "my-secret", "cert-pem", "key-pem");
    let mut secrets = HashMap::new();
    secrets.insert("default/my-secret".to_string(), secret);
    store.replace_all(secrets);

    assert_eq!(store.keys(), vec!["default/my-secret"]);

    let found = store.get(Some("default"), "my-secret");
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.metadata.name.as_deref(), Some("my-secret"));
    assert_eq!(found.metadata.namespace.as_deref(), Some("default"));

    let data = found.data.as_ref().unwrap();
    let cert = data.get(CERT).unwrap();
    assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "cert-pem");
}

#[test]
fn test_secret_store_update() {
    let store = SecretStore::new();

    let secret1 = create_test_secret("prod", "cert-1", "cert1", "key1");
    let mut initial = HashMap::new();
    initial.insert("prod/cert-1".to_string(), secret1);
    store.replace_all(initial);

    let secret2 = create_test_secret("prod", "cert-2", "cert2", "key2");
    let mut upsert = HashMap::new();
    upsert.insert("prod/cert-2".to_string(), secret2);
    store.update(upsert, &HashSet::new());

    assert!(store.get(Some("prod"), "cert-1").is_some());
    assert!(store.get(Some("prod"), "cert-2").is_some());

    let secret1_updated = create_test_secret("prod", "cert-1", "updated-cert", "updated-key");
    let mut upsert = HashMap::new();
    upsert.insert("prod/cert-1".to_string(), secret1_updated);
    store.update(upsert, &HashSet::new());

    let found = store.get(Some("prod"), "cert-1").unwrap();
    let data = found.data.as_ref().unwrap();
    let cert = data.get(CERT).unwrap();
    assert_eq!(String::from_utf8(cert.0.clone()).unwrap(), "updated-cert");

    let mut remove = HashSet::new();
    remove.insert("prod/cert-2".to_string());
    store.update(HashMap::new(), &remove);

    assert!(store.get(Some("prod"), "cert-2").is_none());
    assert!(store.get(Some("prod"), "cert-1").is_some());
}

#[test]
fn test_secret_store_no_namespace() {
    let store = SecretStore::new();

    let secret = create_test_secret("", "cluster-secret", "cert", "key");
    let mut secrets = HashMap::new();
    secrets.insert("cluster-secret".to_string(), secret);
    store.replace_all(secrets);

    let found = store.get(None, "cluster-secret");
    assert!(found.is_some());
}
