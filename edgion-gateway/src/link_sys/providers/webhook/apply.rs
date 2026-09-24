//! Webhook fingerprint so equal-runtime relists skip prepare/commit.

use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use crate::link_sys::runtime::apply::{hash_tls_material, FingerprintHasher, RuntimeFingerprint};

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
                current_status: None,
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
}
