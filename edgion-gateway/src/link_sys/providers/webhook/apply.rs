//! Webhook fingerprint so equal-runtime relists skip prepare/commit.

use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use edgion_common::link_sys_core::fingerprint::hash_tls_inheritance;
use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;

use crate::link_sys::runtime::apply::{hash_tls_material, FingerprintHasher, RuntimeFingerprint};

pub(super) fn fingerprint_webhook(
    ls: &LinkSys,
    defaults: &GlobalOutboundTlsSnapshot,
) -> anyhow::Result<RuntimeFingerprint> {
    let SystemConfig::Webhook(cfg) = &ls.spec.config else {
        anyhow::bail!("fingerprint_webhook called with a non-Webhook LinkSys");
    };

    let mut secret = FingerprintHasher::new();
    hash_resolved_secrets(&mut secret, cfg.resolved_secrets.as_ref());
    hash_tls_material(&mut secret, cfg.conn.tls.as_ref().filter(|tls| tls.enabled));
    hash_tls_inheritance(
        &mut secret,
        cfg.conn.tls.as_ref().filter(|tls| tls.enabled).map(|tls| &tls.outbound),
        defaults,
    );

    // Serialize a borrowed redacted view; no Secret/config is cloned to strip it.
    let mut conn = {
        let _redact = edgion_resources::redact::AdminRedactGuard::new();
        serde_json::to_value(&cfg.conn)?
    };
    let fields = conn
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("invalid Webhook fingerprint projection"))?;
    if cfg.conn.tls.as_ref().is_none_or(|tls| !tls.enabled) {
        fields.remove("tls");
    } else if let Some(tls) = fields.get_mut("tls").and_then(serde_json::Value::as_object_mut) {
        tls.remove("resolvedCaCertificates");
        tls.remove("resolvedClientCertificate");
    }
    if cfg.conn.target.url.is_none() && cfg.conn.target.service.namespace.is_none() {
        if let Some(target) = fields.get_mut("target").and_then(serde_json::Value::as_object_mut) {
            target.insert(
                "namespace".into(),
                ls.metadata.namespace.as_deref().unwrap_or("default").into(),
            );
        }
    }

    let mut rebuild = FingerprintHasher::new();
    rebuild.tag(5); // webhook kind tag
    rebuild.bytes(&serde_json::to_vec(&conn)?);
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
