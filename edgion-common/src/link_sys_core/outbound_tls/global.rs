//! Immutable actual-owner global TLS aggregation. No detached policy/material store.
use super::{validate_ca_pairs, validate_client_pair, MaterialField, TlsMaterialProofError};
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal, SecretObjectReference};
use edgion_resources::resources::refs::egc_tls_fields::PreparedEgcTlsReferences;
use edgion_resources::EdgionGatewayConfig;
use k8s_openapi::api::core::v1::Secret;
use std::sync::Arc;

struct Candidate {
    owner: Arc<EdgionGatewayConfig>,
    prepared: PreparedEgcTlsReferences,
}
impl Candidate {
    fn policy(&self) -> Option<&OutboundTlsConfig> {
        self.owner.spec.outbound_tls.as_ref()
    }
    fn source_proof(&self) -> Result<(), TlsMaterialProofError> {
        self.prepared
            .bind(&self.owner)
            .map(|_| ())
            .map_err(TlsMaterialProofError::Reference)
    }
    fn proof(&self, client: bool) -> Result<(), TlsMaterialProofError> {
        let binding = self
            .prepared
            .bind(&self.owner)
            .map_err(TlsMaterialProofError::Reference)?;
        let field = binding.outbound_tls().map_err(TlsMaterialProofError::Reference)?;
        let Some(field) = field else {
            return Ok(());
        };
        let field = MaterialField::Global(&field);
        if client {
            validate_client_pair(&field)
        } else {
            validate_ca_pairs(&field)
        }
    }
}

/// Global defaults retain original immutable EGC owners. Construction cannot
/// accept raw policy/material pairs or upgrade ordinary reference tokens.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
/// let snapshot = GlobalOutboundTlsSnapshot {};
/// ```
pub struct GlobalOutboundTlsSnapshot {
    candidates: Vec<Candidate>,
    verify: bool,
    verify_proof: Result<(), TlsMaterialProofError>,
    ca: Option<usize>,
    client: Option<usize>,
    ca_conflict: bool,
    client_conflict: bool,
    ca_proof: Result<(), TlsMaterialProofError>,
    client_proof: Result<(), TlsMaterialProofError>,
}
impl std::fmt::Debug for GlobalOutboundTlsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalOutboundTlsSnapshot").finish_non_exhaustive()
    }
}
impl GlobalOutboundTlsSnapshot {
    pub fn from_configs(configs: impl IntoIterator<Item = Arc<EdgionGatewayConfig>>) -> Self {
        let mut candidates: Vec<_> = configs
            .into_iter()
            .map(|owner| Candidate {
                prepared: PreparedEgcTlsReferences::from_config(&owner),
                owner,
            })
            .collect();
        // Stable selection of fixed proof failures, independently of HashMap order.
        candidates.sort_by(|a, b| {
            (&a.owner.metadata.name, &a.owner.metadata.namespace)
                .cmp(&(&b.owner.metadata.name, &b.owner.metadata.namespace))
        });
        let verify = candidates.is_empty()
            || candidates
                .iter()
                .any(|candidate| candidate.policy().is_none_or(|p| p.verify));
        let verify_proof = candidates.iter().try_for_each(Candidate::source_proof);
        let mut ca = None;
        let mut ca_conflict = false;
        let mut client: Option<usize> = None;
        let mut client_conflict = false;
        for (index, candidate) in candidates.iter().enumerate() {
            let Some(policy) = candidate.policy() else {
                continue;
            };
            if policy.validation.as_ref().is_some_and(|v| {
                v.ca_certificate_refs.as_ref().is_some_and(|r| !r.is_empty()) || v.well_known_ca_certificates.is_some()
            }) {
                if ca.is_some() {
                    ca_conflict = true;
                }
                ca = Some(index);
            }
            if let Some(reference) = policy.client_certificate_ref.as_ref() {
                if let Some(previous) = client {
                    client_conflict = true;
                    let previous_ref = candidates[previous]
                        .policy()
                        .and_then(|policy| policy.client_certificate_ref.as_ref())
                        .expect("client index is assigned only for immutable candidates with a declared TLS client reference");
                    if client_sort_key(reference) < client_sort_key(previous_ref) {
                        client = Some(index);
                    }
                } else {
                    client = Some(index);
                }
            }
        }
        if ca_conflict {
            ca = None;
        }
        let ca_proof = ca.map_or(Ok(()), |index| candidates[index].proof(false));
        let client_proof = if client_conflict {
            Err(TlsMaterialProofError::ClientPresence)
        } else {
            client.map_or(Ok(()), |index| candidates[index].proof(true))
        };
        Self {
            candidates,
            verify,
            verify_proof,
            ca,
            client,
            ca_conflict,
            client_conflict,
            ca_proof,
            client_proof,
        }
    }
    pub fn verify(&self) -> bool {
        self.verify
    }
    pub fn ca_conflict(&self) -> bool {
        self.ca_conflict
    }
    pub fn client_conflict(&self) -> bool {
        self.client_conflict
    }
    pub fn validation(&self) -> Option<&OutboundTlsValidation> {
        self.ca
            .and_then(|index| self.candidates[index].policy())
            .and_then(|p| p.validation.as_ref())
    }
    pub fn ca_material(&self) -> Option<&[Secret]> {
        self.ca
            .and_then(|index| self.candidates[index].policy())
            .and_then(|p| p.resolved_ca_certificates.as_deref())
    }
    pub fn client_reference(&self) -> Option<&SecretObjectReference> {
        self.client
            .and_then(|index| self.candidates[index].policy())
            .and_then(|p| p.client_certificate_ref.as_ref())
    }
    pub fn client_material(&self) -> Option<&Secret> {
        if self.client_conflict {
            return None;
        }
        self.client
            .and_then(|index| self.candidates[index].policy())
            .and_then(|p| p.resolved_client_certificate.as_ref())
    }
    // Axis precedence is applied before failure propagation. Declaration counts
    // include malformed inputs, while each inherited axis proves its actual owner.
    pub(crate) fn effective_proofs(&self, local: &OutboundTlsLocal) -> [Result<(), TlsMaterialProofError>; 3] {
        self.effective_proofs_for(
            local.verify.is_some(),
            local.validation.is_some(),
            local.client_certificate_ref.is_some(),
        )
    }
    pub(super) fn effective_proofs_for(
        &self,
        local_verify: bool,
        local_validation: bool,
        local_client: bool,
    ) -> [Result<(), TlsMaterialProofError>; 3] {
        [
            if local_verify { Ok(()) } else { self.verify_proof },
            if local_validation { Ok(()) } else { self.ca_proof },
            if local_client { Ok(()) } else { self.client_proof },
        ]
    }
    pub(super) fn effective_eq(&self, other: &Self) -> bool {
        self.verify == other.verify
            && self.verify_proof == other.verify_proof
            && self.ca_conflict == other.ca_conflict
            && self.client_conflict == other.client_conflict
            && self.ca_proof == other.ca_proof
            && self.client_proof == other.client_proof
            && self.validation() == other.validation()
            && self.ca_material() == other.ca_material()
            && self.client_reference() == other.client_reference()
            && self.client_material() == other.client_material()
    }
}
fn client_sort_key(reference: &SecretObjectReference) -> (&str, &str, &str, &str) {
    (
        reference.namespace.as_deref().unwrap_or_default(),
        &reference.name,
        reference.group.as_deref().unwrap_or_default(),
        reference.kind.as_deref().unwrap_or("Secret"),
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys_core/outbound_tls/global_test.rs"]
mod tests;
