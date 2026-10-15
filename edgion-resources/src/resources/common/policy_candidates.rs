//! Pure prepared-policy eligibility and cold arbitration shared by both binaries.
use super::policy_precedence::policy_precedence_key;
use crate::resources::backend_tls_policy::backend_tls_identity_validation_errors;
use crate::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass};
use crate::resources::refs::policy_fields::{BackendTlsPolicyTargets, BackendTrafficPolicyTargets};
use crate::{BackendTLSPolicy, EdgionBackendTrafficPolicy};

fn valid_tls_candidate(policy: &BackendTLSPolicy) -> bool {
    if policy.spec.prepared_precedence.is_none() {
        return false;
    }
    if matches!(
        policy.spec.validation.well_known_ca_certificates.as_ref(),
        Some(crate::resources::backend_tls_policy::WellKnownCACertificates::Other(_))
    ) {
        return false;
    }
    if policy.spec.target_refs.len() != 1 {
        return false;
    }
    let Some(resolved_class) = policy.spec.resolved_target_class else {
        return false;
    };
    let Some(field) = BackendTlsPolicyTargets::from_policy(policy).fields().next() else {
        return false;
    };
    let Ok(key) = field.canonical_key() else {
        return false;
    };
    let class_matches = matches!(
        (resolved_class, key.target_kind),
        (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
            | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
    );
    if !class_matches {
        return false;
    }
    if field.section_name().is_some() && key.target_kind != PolicyTargetKind::Service {
        return false;
    }
    if !backend_tls_identity_validation_errors(&policy.spec.validation).is_empty() {
        return false;
    }
    crate::resources::refs::backend_tls_fields::BackendTlsRefs::from_policy(policy).all_materials_resolved()
}

pub fn prepared_backend_tls_target(policy: &BackendTLSPolicy) -> Option<PolicyTargetKey> {
    valid_tls_candidate(policy)
        .then(|| {
            BackendTlsPolicyTargets::from_policy(policy)
                .fields()
                .next()?
                .canonical_key()
                .ok()
        })
        .flatten()
}

pub fn prepared_backend_traffic_targets(policy: &EdgionBackendTrafficPolicy) -> Option<Vec<PolicyTargetKey>> {
    let mut targets = BackendTrafficPolicyTargets::from_policy(policy)
        .fields()
        .map(|field| field.canonical_key())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    targets.sort();
    targets.dedup();
    let class = policy.spec.resolved_target_class?;
    (policy.spec.prepared_precedence.is_some()
        && policy.spec.get_validation_errors().is_empty()
        && !targets.is_empty()
        && targets.iter().all(|target| {
            matches!(
                (class, target.target_kind),
                (ResolvedPolicyTargetClass::Service, PolicyTargetKind::Service)
                    | (ResolvedPolicyTargetClass::Ai, PolicyTargetKind::EdgionBackend)
            )
        }))
    .then_some(targets)
}

#[derive(Clone, Copy)]
pub enum PolicySection<'a> {
    Named(&'a str),
    NumberOnly,
    Unproven,
}
pub enum PolicyCandidate<'a, T> {
    Absent,
    Invalid,
    Effective(&'a T),
}

pub fn select_backend_tls_candidate<'a>(
    policies: impl IntoIterator<Item = &'a BackendTLSPolicy>,
    target: &PolicyTargetKey,
    section: PolicySection<'_>,
) -> PolicyCandidate<'a, BackendTLSPolicy> {
    let mut exact: Option<&BackendTLSPolicy> = None;
    let mut whole: Option<&BackendTLSPolicy> = None;
    let mut invalid_exact = false;
    let mut invalid_whole = false;
    let mut any_section = false;
    let older = |a: &BackendTLSPolicy, b: &BackendTLSPolicy| {
        policy_precedence_key(
            a.spec.prepared_precedence.and_then(|p| p.creation_timestamp),
            a.metadata.name.as_deref().unwrap_or_default(),
        ) < policy_precedence_key(
            b.spec.prepared_precedence.and_then(|p| p.creation_timestamp),
            b.metadata.name.as_deref().unwrap_or_default(),
        )
    };
    for policy in policies {
        let valid = prepared_backend_tls_target(policy).as_ref() == Some(target);
        for field in BackendTlsPolicyTargets::from_policy(policy).fields() {
            if field.canonical_key().as_ref() != Ok(target) {
                continue;
            }
            let scoped = field.section_name().filter(|name| !name.is_empty());
            any_section |= scoped.is_some();
            let (winner, invalid) = if let Some(name) = scoped {
                if !matches!(section, PolicySection::Named(selected) if selected == name) {
                    continue;
                }
                (&mut exact, &mut invalid_exact)
            } else {
                (&mut whole, &mut invalid_whole)
            };
            if valid {
                if winner.is_none_or(|previous| older(policy, previous)) {
                    *winner = Some(policy);
                }
            } else {
                *invalid = true;
            }
        }
    }
    if matches!(section, PolicySection::Unproven) && any_section {
        return PolicyCandidate::Invalid;
    }
    if let Some(policy) = exact {
        return PolicyCandidate::Effective(policy);
    }
    if invalid_exact {
        return PolicyCandidate::Invalid;
    }
    if let Some(policy) = whole {
        return PolicyCandidate::Effective(policy);
    }
    if invalid_whole {
        PolicyCandidate::Invalid
    } else {
        PolicyCandidate::Absent
    }
}

pub fn select_backend_traffic_candidate<'a>(
    policies: impl IntoIterator<Item = &'a EdgionBackendTrafficPolicy>,
    target: &PolicyTargetKey,
) -> Option<&'a EdgionBackendTrafficPolicy> {
    policies
        .into_iter()
        .filter(|policy| prepared_backend_traffic_targets(policy).is_some_and(|targets| targets.contains(target)))
        .min_by(|a, b| {
            policy_precedence_key(
                a.spec.prepared_precedence.and_then(|p| p.creation_timestamp),
                a.metadata.name.as_deref().unwrap_or_default(),
            )
            .cmp(&policy_precedence_key(
                b.spec.prepared_precedence.and_then(|p| p.creation_timestamp),
                b.metadata.name.as_deref().unwrap_or_default(),
            ))
        })
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/policy_candidates_test.rs"]
mod tests;
