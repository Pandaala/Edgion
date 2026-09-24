use super::extract_dockerconfigjson;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;

fn secret(type_: Option<&str>, key: Option<(&str, &[u8])>) -> Secret {
    let mut s = Secret {
        type_: type_.map(str::to_string),
        ..Default::default()
    };
    if let Some((k, v)) = key {
        s.data = Some([(k.to_string(), ByteString(v.to_vec()))].into_iter().collect());
    }
    s
}

#[test]
fn accepts_wellformed_dockerconfigjson_secret() {
    let s = secret(
        Some("kubernetes.io/dockerconfigjson"),
        Some((".dockerconfigjson", br#"{"auths":{}}"#)),
    );
    assert_eq!(extract_dockerconfigjson(&s).unwrap(), r#"{"auths":{}}"#);
}

#[test]
fn rejects_wrong_type_and_missing_key() {
    let s = secret(Some("Opaque"), Some((".dockerconfigjson", b"{}")));
    assert!(extract_dockerconfigjson(&s).unwrap_err().contains("dockerconfigjson"));
    let s = secret(Some("kubernetes.io/dockerconfigjson"), None);
    assert!(extract_dockerconfigjson(&s).unwrap_err().contains(".dockerconfigjson"));
}
