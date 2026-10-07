//! Canonical private fingerprint encoding for outbound policy and consumed TLS material.

use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType, WellKnownCACertificates};
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, TlsConfig};
use k8s_openapi::api::core::v1::Secret;
use sha2::{Digest, Sha256};

use super::outbound_tls::{GlobalOutboundTlsSnapshot, TlsMaterialProofError};

/// Encode only the inherited axes consumed by the actual local policy. None
/// denotes native inactive transport; callers separately prove owning authority.
pub fn hash_tls_inheritance(
    h: &mut FingerprintHasher,
    local: Option<&OutboundTlsLocal>,
    global: &GlobalOutboundTlsSnapshot,
) {
    let Some(local) = local else {
        h.tag(0);
        return;
    };
    h.tag(1);
    let proofs = global.effective_proofs(local);
    h.tag(0); // verify axis
    if local.verify.is_none() {
        h.tag(1);
        h.opt_bool(Some(global.verify()));
        hash_proof(h, proofs[0]);
    } else {
        h.tag(0);
    }
    h.tag(1); // whole validation/CA axis
    if local.validation.is_none() {
        h.tag(1);
        hash_outbound_policy(h, None, global.validation(), None);
        hash_secrets(h, global.ca_material());
        hash_proof(h, proofs[1]);
    } else {
        h.tag(0);
    }
    h.tag(2); // client identity axis
    if local.client_certificate_ref.is_none() {
        h.tag(1);
        hash_outbound_policy(h, None, None, global.client_reference());
        hash_secret(h, global.client_material());
        hash_proof(h, proofs[2]);
    } else {
        h.tag(0);
    }
}

fn hash_proof(h: &mut FingerprintHasher, proof: Result<(), TlsMaterialProofError>) {
    match proof {
        Ok(()) => h.tag(0),
        Err(error) => {
            h.tag(1);
            h.bytes(error.to_string().as_bytes());
        }
    }
}

// Keep shared TLS policy and consumed Secret material encoding in one place.
// Provider-specific rebuild and credential fields remain with each owner.
pub fn hash_tls_policy(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
    let Some(tls) = tls else {
        h.tag(0);
        return;
    };
    h.tag(1);
    h.tag(u8::from(tls.enabled));
    if !tls.enabled {
        return;
    }
    hash_outbound_policy(
        h,
        tls.outbound.verify,
        tls.outbound.validation.as_ref(),
        tls.outbound.client_certificate_ref.as_ref(),
    );
}

pub(crate) fn hash_outbound_policy(
    h: &mut FingerprintHasher,
    verify: Option<bool>,
    validation: Option<&OutboundTlsValidation>,
    client: Option<&SecretObjectReference>,
) {
    h.opt_bool(verify);
    match validation {
        None => h.tag(0),
        Some(validation) => {
            h.tag(1);
            h.opt_str(validation.hostname.as_deref());
            match validation.well_known_ca_certificates {
                None => h.tag(0),
                Some(WellKnownCACertificates::System) => h.tag(1),
                Some(WellKnownCACertificates::Other(ref name)) => {
                    h.tag(2);
                    h.bytes(name.as_bytes());
                }
            }
            match validation.subject_alt_names.as_deref() {
                None => h.tag(0),
                Some(sans) => {
                    h.tag(1);
                    h.u64(sans.len() as u64);
                    for san in sans {
                        hash_san(h, san);
                    }
                }
            }
            match validation.ca_certificate_refs.as_deref() {
                None => h.tag(0),
                Some(refs) => {
                    h.tag(1);
                    h.u64(refs.len() as u64);
                    for r in refs {
                        h.bytes(r.group.as_deref().unwrap_or_default().as_bytes());
                        h.bytes(r.kind.as_bytes());
                        h.bytes(r.name.as_bytes());
                        h.opt_str(r.namespace.as_deref());
                    }
                }
            }
        }
    }
    match client {
        None => h.tag(0),
        Some(r) => {
            h.tag(1);
            h.opt_str(r.group.as_deref());
            h.opt_str(r.kind.as_deref());
            h.bytes(r.name.as_bytes());
            h.opt_str(r.namespace.as_deref());
        }
    }
}

fn hash_san(h: &mut FingerprintHasher, san: &SubjectAltName) {
    h.tag(match san.san_type {
        SubjectAltNameType::Hostname => 0,
        SubjectAltNameType::URI => 1,
    });
    h.opt_str(san.hostname.as_deref());
    h.opt_str(san.uri.as_deref());
}

pub fn hash_tls_material(h: &mut FingerprintHasher, tls: Option<&TlsConfig>) {
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

pub(crate) fn hash_secrets(h: &mut FingerprintHasher, secrets: Option<&[Secret]>) {
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

pub(crate) fn hash_secret(h: &mut FingerprintHasher, secret: Option<&Secret>) {
    match secret {
        None => h.tag(0),
        Some(secret) => {
            h.tag(1);
            hash_secret_data(h, secret);
        }
    }
}

fn hash_secret_data(h: &mut FingerprintHasher, secret: &Secret) {
    // Owning material proof consumes these fields, so byte-identical bundles
    // with different identities or source kinds must not share a fingerprint.
    // UID and resourceVersion remain excluded from material identity.
    h.opt_str(secret.metadata.name.as_deref());
    h.opt_str(secret.metadata.namespace.as_deref());
    h.opt_str(
        secret
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(CONTROLLER_CA_SOURCE).map(String::as_str)),
    );
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

/// Length-prefixed SHA-256 hasher for rebuild / secret fingerprints.
pub struct FingerprintHasher(Sha256);

impl FingerprintHasher {
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    pub fn tag(&mut self, tag: u8) {
        self.0.update([tag]);
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value);
    }

    pub fn u64(&mut self, value: u64) {
        self.0.update(value.to_le_bytes());
    }

    pub fn opt_str(&mut self, value: Option<&str>) {
        match value {
            None => self.tag(0),
            Some(s) => {
                self.tag(1);
                self.bytes(s.as_bytes());
            }
        }
    }

    pub fn opt_bool(&mut self, value: Option<bool>) {
        match value {
            None => self.tag(0),
            Some(false) => self.tag(1),
            Some(true) => self.tag(2),
        }
    }

    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl Default for FingerprintHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/fingerprint_test.rs"]
mod tests;
