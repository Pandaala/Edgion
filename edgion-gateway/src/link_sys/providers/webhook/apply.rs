//! Webhook fingerprint so equal-runtime relists skip prepare/commit.

use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use k8s_openapi::api::core::v1::Secret;

use crate::link_sys::runtime::apply::{FingerprintHasher, RuntimeFingerprint};

pub fn fingerprint_webhook(ls: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Webhook(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_webhook called with a non-Webhook LinkSys");
    };

    let mut cfg = cfg.clone();
    // Match prepare(): only enabled TLS reaches the outbound client, and a
    // service target without namespace inherits the LinkSys namespace.
    cfg.conn.tls = cfg.conn.tls.take().filter(|tls| tls.enabled);
    if cfg.conn.target.url.is_none() && cfg.conn.target.service.namespace.is_none() {
        if let Some(ns) = ls.metadata.namespace.as_deref() {
            cfg.conn.target.service.namespace = Some(ns.to_string());
        }
    }

    let mut secret = FingerprintHasher::new();
    hash_resolved_secrets(&mut secret, cfg.resolved_secrets.as_ref());
    hash_tls_material(&mut secret, cfg.conn.tls.as_ref());

    if let Some(tls) = cfg.conn.tls.as_mut() {
        tls.resolved_ca_certificates = None;
        tls.resolved_client_certificate = None;
    }

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(5); // webhook kind tag
    let conn = serde_json::to_vec(&cfg.conn)?;
    rebuild.bytes(&conn);
    let request = serde_json::to_vec(&cfg.request)?;
    rebuild.bytes(&request);

    Ok(RuntimeFingerprint::from_parts(
        rebuild.finish(),
        secret.finish(),
        ls.metadata.resource_version.clone(),
    ))
}

fn hash_resolved_secrets(h: &mut FingerprintHasher, secrets: Option<&ResolvedSecrets>) {
    match secrets {
        None => h.tag(0),
        Some(ResolvedSecrets(map)) => {
            h.tag(1);
            h.u64(map.len() as u64);
            for (key, value) in map {
                h.bytes(key.as_bytes());
                h.bytes(value.as_bytes());
            }
        }
    }
}

fn hash_tls_material(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    if !tls.enabled {
        return;
    }
    hash_secrets(h, tls.resolved_ca_certificates.as_deref());
    hash_secret(h, tls.resolved_client_certificate.as_ref());
}

fn hash_secrets(h: &mut FingerprintHasher, secrets: Option<&[Secret]>) {
    match secrets {
        None => h.tag(0),
        Some(secrets) => {
            h.tag(1);
            h.u64(secrets.len() as u64);
            for secret in secrets {
                hash_secret_data(h, secret);
            }
        }
    }
}

fn hash_secret(h: &mut FingerprintHasher, secret: Option<&Secret>) {
    match secret {
        None => h.tag(0),
        Some(secret) => {
            h.tag(1);
            hash_secret_data(h, secret);
        }
    }
}

fn hash_secret_data(h: &mut FingerprintHasher, secret: &Secret) {
    let mut entries: Vec<(&String, &k8s_openapi::ByteString)> = secret
        .data
        .as_ref()
        .map(|data| data.iter().collect())
        .unwrap_or_default();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    h.u64(entries.len() as u64);
    for (key, value) in entries {
        h.bytes(key.as_bytes());
        h.bytes(&value.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };
    use edgion_resources::resources::link_sys::LinkSysSpec;

    fn webhook_ls(port: i32, rv: Option<&str>) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some("fp".to_string()),
                name: Some("wh".to_string()),
                resource_version: rv.map(str::to_string),
                ..Default::default()
            },
            spec: LinkSysSpec {
                unknown_fields: Vec::new(),
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    conn: ExternalHttpConnection {
                        target: WebhookTarget {
                            service: WebhookServiceRef {
                                name: Some("svc".to_string()),
                                namespace: Some("fp".to_string()),
                                port: Some(port),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[test]
    fn equal_port_matches() {
        let a = webhook_ls(8080, Some("1"));
        let b = webhook_ls(8080, Some("2"));
        assert!(fingerprint_webhook(&a)
            .unwrap()
            .equal_runtime(&fingerprint_webhook(&b).unwrap()));
    }

    #[test]
    fn port_change_rebuilds() {
        let a = webhook_ls(8080, None);
        let b = webhook_ls(8081, None);
        assert!(!fingerprint_webhook(&a)
            .unwrap()
            .equal_runtime(&fingerprint_webhook(&b).unwrap()));
    }

    #[test]
    fn ignored_auth_shim_does_not_change_fingerprint() {
        let mut a = webhook_ls(8080, None);
        let b = a.clone();
        if let SystemConfig::Webhook(cfg) = &mut a.spec.config {
            cfg.auth = Some(serde_json::json!({"type": "bearer"}));
        }
        assert!(fingerprint_webhook(&a)
            .unwrap()
            .equal_runtime(&fingerprint_webhook(&b).unwrap()));
    }
}
