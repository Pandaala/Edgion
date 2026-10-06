//! Webhook fingerprint so equal-runtime relists skip prepare/commit.

use edgion_resources::resources::common::ResolvedSecrets;
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
#[path = "../../../../tests/unit/link_sys/providers/webhook/apply_test.rs"]
mod tests;
