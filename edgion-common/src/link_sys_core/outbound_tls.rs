//! Shared outbound TLS resolver for all outbound callers (LinkSys providers,
//! plugins, etc.).
//!
//! Provides a single entry point [`resolve_outbound_tls`] that:
//! - inherits the cluster-wide default from [`outbound_global_default`]
//! - applies per-call overrides from [`OutboundTlsLocal`]
//! - resolves CA material from resolved Kubernetes Secrets (one per ref)

use std::sync::{Arc, LazyLock};

use edgion_resources::constants::secret_keys::tls as tls_keys;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
use edgion_resources::EdgionErrorCode;
use k8s_openapi::api::core::v1::Secret;
use parking_lot::RwLock;
use zeroize::Zeroizing;

use edgion_resources::resources::refs::outbound_tls::{
    BoundGlobalOutboundTls, BoundLocalTlsSelection, BoundOutboundTls, BoundTlsMaterial,
};
use edgion_resources::resources::refs::ReferenceError;

mod global;
pub use global::GlobalOutboundTlsSnapshot;
mod link_sys;
pub use link_sys::PreparedLinkSysTls;
mod redis;
pub use redis::PreparedRedisTls;
mod otlp;
pub use otlp::PreparedOtlpTls;

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/outbound_tls_test/test_util.rs"]
pub mod test_util;
#[cfg(all(not(test), feature = "test-util"))]
#[path = "../../tests/unit/link_sys_core/outbound_tls_test/test_util.rs"]
pub mod test_util;

/// Local field identity/material pairing failure. Values never contain material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMaterialProofError {
    Reference(ReferenceError),
    CaCardinality,
    ClientPresence,
    MaterialIdentity,
    MaterialProvenance,
}
impl std::fmt::Display for TlsMaterialProofError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Reference(_) => "invalid outbound TLS reference",
            Self::CaCardinality => "outbound TLS CA reference/material cardinality mismatch",
            Self::ClientPresence => "outbound TLS client reference/material presence mismatch",
            Self::MaterialIdentity => "outbound TLS reference/material identity mismatch",
            Self::MaterialProvenance => "outbound TLS reference/material provenance mismatch",
        })
    }
}
impl std::error::Error for TlsMaterialProofError {}

/// A proved local field. Inherited global CA/client axes require separate proof.
/// This does not parse PEM, authorize grants or construct a transport.
///
/// Proof cannot overlap mutable access to its actual field.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_common::link_sys_core::outbound_tls::validate_outbound_tls_field;
/// let mut owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "metadata":{"name":"plugins"}, "spec":{"requestPlugins":[{
///         "type":"LdapAuth", "config":{"ldapHost":"ldap.example","baseDn":"dc=example","attribute":"uid"}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let mut binding = prepared.bind_mut(&mut owner).unwrap();
/// let mut entry = binding.request_entries().next().unwrap();
/// let proof = validate_outbound_tls_field(entry.outbound_tls().unwrap().unwrap()).unwrap();
/// let _ = entry.config_mut();
/// let _ = proof.field().policy();
/// ```
/// A proof cannot be constructed externally.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::ValidatedLocalOutboundTls;
/// let proof = ValidatedLocalOutboundTls {};
/// ```
/// The actual owner cannot be dropped while its proof is still used.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// use edgion_common::link_sys_core::outbound_tls::validate_outbound_tls_field;
/// let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
///     "metadata":{"name":"plugins"}, "spec":{"requestPlugins":[{
///         "type":"LdapAuth", "config":{"ldapHost":"ldap.example","baseDn":"dc=example","attribute":"uid"}
///     }]}
/// })).unwrap();
/// let prepared = PreparedPluginReferences::from_plugins(&owner);
/// let binding = prepared.bind(&owner).unwrap();
/// let entry = binding.request_entries().next().unwrap();
/// let proof = validate_outbound_tls_field(entry.outbound_tls().unwrap().unwrap()).unwrap();
/// drop(owner);
/// let _ = proof.field().policy();
/// ```
pub struct ValidatedLocalOutboundTls<'a> {
    field: BoundOutboundTls<'a>,
}
impl std::fmt::Debug for ValidatedLocalOutboundTls<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedLocalOutboundTls")
            .finish_non_exhaustive()
    }
}
impl ValidatedLocalOutboundTls<'_> {
    pub fn field(&self) -> &BoundOutboundTls<'_> {
        &self.field
    }
}

/// Check the entire actual active field before any eventual client/replay use.
/// Keep author order and all declared slots, even unsupported LinkSys CA kinds.
pub fn validate_outbound_tls_field(
    field: BoundOutboundTls<'_>,
) -> Result<ValidatedLocalOutboundTls<'_>, TlsMaterialProofError> {
    field.validate_source().map_err(TlsMaterialProofError::Reference)?;
    validate_local_material(&field)?;
    Ok(ValidatedLocalOutboundTls { field })
}

fn validate_local_material(field: &BoundOutboundTls<'_>) -> Result<(), TlsMaterialProofError> {
    let loan = MaterialField::Local(field);
    validate_ca_pairs(&loan)?;
    validate_client_pair(&loan)
}

/// Proved actual selection, including absence/disablement. This is not transport
/// authority; owning client adapters retain their existing enable/scheme gates.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::ValidatedLocalTlsSelection;
/// let proof = ValidatedLocalTlsSelection {};
/// ```
/// Inactive local policy does not permit mutation under its proof.
/// ```compile_fail
/// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
/// use edgion_common::link_sys_core::outbound_tls::validate_local_tls_selection;
/// fn overlap(prepared: &PreparedLinkSysReferences, owner: &mut edgion_resources::LinkSys) {
///     let mut binding = prepared.bind_mut(owner).unwrap();
///     let proof = validate_local_tls_selection(binding.outbound_tls_selection().unwrap()).unwrap();
///     binding.config_mut();
///     proof.active_field();
/// }
/// ```
pub struct ValidatedLocalTlsSelection<'a> {
    selection: BoundLocalTlsSelection<'a>,
}
impl std::fmt::Debug for ValidatedLocalTlsSelection<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedLocalTlsSelection")
            .finish_non_exhaustive()
    }
}
impl ValidatedLocalTlsSelection<'_> {
    pub fn active_field(&self) -> Option<&BoundOutboundTls<'_>> {
        self.selection.active_field()
    }
}

/// Check source identity before optional material branches; no raw input can
/// replace the actual selection in the resulting proof.
pub fn validate_local_tls_selection(
    selection: BoundLocalTlsSelection<'_>,
) -> Result<ValidatedLocalTlsSelection<'_>, TlsMaterialProofError> {
    selection.validate_source().map_err(TlsMaterialProofError::Reference)?;
    if let Some(field) = selection.active_field() {
        validate_local_material(field)?;
    }
    Ok(ValidatedLocalTlsSelection { selection })
}

/// Pure inheritance check for transports with active local TLS. Native inactive
/// HTTP transports consume no global defaults. This does no PEM parsing or audit.
pub fn validate_active_tls_inheritance(
    proof: &ValidatedLocalTlsSelection<'_>,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<(), TlsMaterialProofError> {
    if let Some(field) = proof.active_field() {
        for axis in global.effective_proofs(field.policy()) {
            axis?;
        }
    }
    Ok(())
}

/// Pure effective preparation against the exact captured global generation.
/// The caller's actual owning adapter decides whether its transport uses TLS.
pub fn resolve_validated_outbound_tls(
    proof: &ValidatedLocalTlsSelection<'_>,
    provider_can_disable: bool,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let empty = OutboundTlsLocal::default();
    let field = proof.active_field();
    resolve_outbound_tls_with_snapshot(
        field.map_or(&empty, BoundOutboundTls::policy),
        field.and_then(BoundOutboundTls::ca_material),
        field.and_then(BoundOutboundTls::client_material),
        provider_can_disable,
        global,
    )
}

// Closed adapters retain the actual owning loans; there is no raw-pair proof API.
enum MaterialField<'a, 'owner> {
    Local(&'a BoundOutboundTls<'owner>),
    Global(&'a BoundGlobalOutboundTls<'owner>),
}
impl MaterialField<'_, '_> {
    fn ca_count(&self) -> usize {
        match self {
            Self::Local(field) => field.ca_reference_count(),
            Self::Global(field) => field.ca_reference_count(),
        }
    }
    fn ca_material(&self) -> Option<&[Secret]> {
        match self {
            Self::Local(field) => field.ca_material(),
            Self::Global(field) => field.ca_material(),
        }
    }
    fn visit_ca<'a>(&'a self, visit: impl FnMut(BoundTlsMaterial<'a>)) {
        match self {
            Self::Local(field) => field.visit_ca(visit),
            Self::Global(field) => field.visit_ca(visit),
        }
    }
    fn client(&self) -> Option<BoundTlsMaterial<'_>> {
        match self {
            Self::Local(field) => field.client_certificate(),
            Self::Global(field) => field.client_certificate(),
        }
    }
    fn client_material(&self) -> Option<&Secret> {
        match self {
            Self::Local(field) => field.client_material(),
            Self::Global(field) => field.client_material(),
        }
    }
}
fn validate_ca_pairs(field: &MaterialField<'_, '_>) -> Result<(), TlsMaterialProofError> {
    let expected = field.ca_count();
    if field.ca_material().map_or(0, <[Secret]>::len) != expected {
        return Err(TlsMaterialProofError::CaCardinality);
    }
    let mut count = 0;
    let mut result = Ok(());
    field.visit_ca(|pair| {
        count += 1;
        if result.is_ok() {
            result = validate_material_pair(pair);
        }
    });
    result?;
    if count != expected {
        return Err(TlsMaterialProofError::CaCardinality);
    }
    Ok(())
}
fn validate_client_pair(field: &MaterialField<'_, '_>) -> Result<(), TlsMaterialProofError> {
    match (field.client(), field.client_material()) {
        (Some(pair), Some(_)) => validate_material_pair(pair)?,
        (None, None) => {}
        _ => return Err(TlsMaterialProofError::ClientPresence),
    }
    Ok(())
}

fn validate_material_pair(pair: BoundTlsMaterial<'_>) -> Result<(), TlsMaterialProofError> {
    use edgion_resources::resources::refs::outbound_tls::TlsMaterialIdentityError;
    pair.validate_identity().map_err(|error| match error {
        TlsMaterialIdentityError::Reference(error) => TlsMaterialProofError::Reference(error),
        TlsMaterialIdentityError::Identity => TlsMaterialProofError::MaterialIdentity,
        TlsMaterialIdentityError::Provenance => TlsMaterialProofError::MaterialProvenance,
    })
}

// ============================================================================
// Output types
// ============================================================================

/// The effective CA trust source after resolving global defaults and local
/// overrides.
pub enum EffectiveCa {
    /// Built-in webpki roots compiled into the binary.
    System,
    /// PEM bundles that REPLACE the public roots (private CA mode).
    /// Each inner `Vec<u8>` is one complete PEM bundle (may contain multiple
    /// certificates).
    Custom(Vec<Vec<u8>>),
}

/// Resolved outbound TLS configuration, ready for consumption by a provider.
pub struct ResolvedOutboundTls {
    pub verify: bool,
    pub ca: EffectiveCa,
    /// `(cert_pem, key_pem)` when a client-identity Secret was provided.
    pub client_identity: Option<(Vec<u8>, Zeroizing<Vec<u8>>)>,
    /// Effective identity and trust policy. Transports must enforce hostname/SAN
    /// constraints or reject them before dialing.
    pub san: Option<OutboundTlsValidation>,
}

impl ResolvedOutboundTls {
    /// Refuse identity constraints when a transport cannot enforce them.
    /// Call after resolution so inherited validation is checked as well.
    pub(crate) fn reject_unsupported_identity_policy(&self, provider: &str) -> anyhow::Result<()> {
        if let Some(validation) = &self.san {
            anyhow::ensure!(
                validation.hostname.is_none() && validation.subject_alt_names.as_ref().is_none_or(Vec::is_empty),
                "{provider} TLS hostname/subjectAltNames policy is unsupported by this transport"
            );
        }
        Ok(())
    }
}

// ============================================================================
// Cluster-wide default store
// ============================================================================

static OUTBOUND_GLOBAL_DEFAULT: LazyLock<RwLock<Arc<GlobalOutboundTlsSnapshot>>> =
    LazyLock::new(|| RwLock::new(Arc::new(GlobalOutboundTlsSnapshot::from_configs(std::iter::empty()))));

#[cfg(test)]
#[path = "../../tests/support/outbound_tls.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::outbound_tls_test_guard;

/// Returns the cluster-wide outbound TLS default.
///
/// Initialized to `OutboundTlsConfig::default()` (`verify = true`, no CA
/// override). Updated by [`set_outbound_global_default`] when an
/// `EdgionGatewayConfig` is applied.
pub fn outbound_global_default() -> Arc<GlobalOutboundTlsSnapshot> {
    Arc::clone(&OUTBOUND_GLOBAL_DEFAULT.read())
}

/// Replace the cluster-wide outbound TLS default.
///
/// Called by the Gateway when an `EdgionGatewayConfig` carrying an
/// `outboundTls` block is applied. Returns `true` when the effective stored
/// configuration changed.
pub fn set_outbound_global_default(cfg: Arc<GlobalOutboundTlsSnapshot>) -> bool {
    let mut current = OUTBOUND_GLOBAL_DEFAULT.write();
    let changed = !current.effective_eq(&cfg);
    *current = cfg;
    changed
}

// ============================================================================
// Resolver
// ============================================================================

/// The one resolver every outbound caller uses.
///
/// # Arguments
///
/// - `local` — per-call override; all-`None` fields inherit the global default.
/// - `resolved_ca` — the resolved K8s Secrets for the **local**
///   `validation.caCertificateRefs` (one Secret per ref, in the same order).
///   Required when `local.validation` declares CA refs; there is no fallback to
///   the global bundles for a local block (see the precedence section below).
///   Ignored when the local block declares no `validation` (the global block
///   then supplies both refs and resolved bytes).
/// - `resolved_client` — the resolved K8s Secret for client mTLS identity.
/// - `provider_can_disable` — whether this provider exposes a skip-verify knob.
///   Pass `false` for providers like Etcd whose TLS stack always verifies.
/// - `context` — short provider name embedded in log messages.
///
/// # CA source determination
///
/// When `verify == true`, the CA source is determined from the `validation` block
/// (local takes precedence over global):
/// - `caCertificateRefs` present and non-empty → `EffectiveCa::Custom` (multi-bundle)
///   using the resolved Secrets; fails with `OutboundTlsCaMissing` if no resolved
///   Secret carries a valid `tls.ca.crt` PEM.
/// - `wellKnownCACertificates: System` or both absent → `EffectiveCa::System`
///   (built-in webpki roots).
///
/// # Validation-block precedence and global CA fallback
///
/// The `validation` block has **whole-block** precedence: a present
/// `local.validation` shadows the global block entirely (fields are not merged).
/// CA bytes are sourced from whichever block wins:
/// - local block wins → CA bytes come **only** from the per-call `resolved_ca`.
///   If the local block declares `caCertificateRefs` but `resolved_ca` is
///   `None`/empty, this fails with [`EdgionErrorCode::OutboundTlsCaMissing`] rather
///   than silently falling back to the global bundles (which belong to the
///   global refs, not the local ones).
/// - global block wins (no local `validation`) → CA bytes come from the
///   Controller-shipped `global.resolved_ca_certificates`.
///
/// Client-certificate inheritance follows the same ownership rule independently
/// of the validation block: a local `clientCertificateRef` uses only the
/// per-call `resolved_client`; otherwise a configured global ref uses only
/// `global.resolved_client_certificate`. A declared ref without its matching
/// resolved Secret fails closed instead of silently disabling mTLS.
///
/// # Fail-secure: SAN pinning overrides verify=false
///
/// A non-empty `validation.subjectAltNames` policy forces full certificate
/// verification even when the operator set `verify=false` — SAN pinning is
/// meaningless without chain verification. In that case the returned `verify`
/// is `true`, CA material is resolved, and the client mTLS identity is retained.
///
/// # Known limitations
///
/// - M4: When the effective `verify` is `false` (skip-verify actually in force,
///   i.e. no SAN policy raised it), client mTLS identity is intentionally
///   dropped — the skip-verify path sends no client certificate.
///
/// # Operator responsibility for verify=false
///
/// When an operator explicitly sets `verify=false` (and no SAN policy overrides
/// it), it is honored without an environment check — the operator owns this
/// decision. A WARN is emitted at configuration-load time as an audit trail.
/// `verify=false` is rejected ONLY when `provider_can_disable == false` (a
/// technical SDK-capability fact: e.g. etcd-client and the rustls Redis
/// connector have no skip-verify API, so honoring the request would silently
/// still verify).
///
/// # Errors
///
/// - [`EdgionErrorCode::OutboundTlsVerifyUnsupported`] — `verify=false` requested
///   but `provider_can_disable == false`.
/// - [`EdgionErrorCode::OutboundTlsCaMissing`] — `caCertificateRefs` declared but
///   no resolved Secret carries a valid CA PEM bundle.
/// - [`EdgionErrorCode::OutboundTlsClientCertInvalid`] — a declared client ref
///   has no matching resolved Secret, or the effective Secret is missing
///   `tls.crt` or `tls.key`.
pub fn resolve_outbound_tls(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    context: &str,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let global = outbound_global_default();
    resolve_outbound_tls_audited_with_defaults(
        local,
        resolved_ca,
        resolved_client,
        provider_can_disable,
        context,
        &global,
    )
}

/// Cold preparation against one inherited-policy snapshot, retaining canonical audit.
pub(crate) fn resolve_outbound_tls_audited_with_defaults(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    context: &str,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let result = resolve_outbound_tls_with_snapshot(local, resolved_ca, resolved_client, provider_can_disable, global);
    audit_resolution(&result, local.verify.unwrap_or(global.verify()), context);
    result
}

pub(crate) fn resolve_validated_outbound_tls_audited(
    proof: &ValidatedLocalTlsSelection<'_>,
    provider_can_disable: bool,
    context: &str,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let result = resolve_validated_outbound_tls(proof, provider_can_disable, global);
    let requested_verify = proof
        .active_field()
        .and_then(|field| field.policy().verify)
        .unwrap_or(global.verify());
    audit_resolution(&result, requested_verify, context);
    result
}

fn audit_resolution(result: &Result<ResolvedOutboundTls, EdgionErrorCode>, requested_verify: bool, context: &str) {
    match result {
        Err(EdgionErrorCode::OutboundTlsVerifyUnsupported) => {
            tracing::warn!(
                provider = context,
                "outbound TLS verify=false requested but provider does not support skip-verify"
            );
        }
        Ok(resolved) if !resolved.verify => {
            tracing::warn!(
                provider = context,
                "outbound TLS verification disabled (verify=false) — operator-configured, insecure"
            );
        }
        Ok(_) if !requested_verify => {
            tracing::warn!(
                provider = context,
                "outbound TLS verify=false overridden by subjectAltNames policy — full certificate verification enforced (fail-secure)"
            );
        }
        _ => {}
    }
}

/// Resolve outbound TLS against an explicit defaults snapshot.
///
/// Backend-policy consumers pass empty defaults to avoid inheriting unrelated
/// LinkSys trust or client credentials. Ordinary outbound clients use
/// `resolve_outbound_tls` to retain process-default inheritance.
/// This pure resolver emits no logs; request-path callers own their diagnostics.
pub fn resolve_outbound_tls_with_defaults(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    global: &OutboundTlsConfig,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    resolve_effective_tls(
        LocalInputs::resource(local, resolved_ca, resolved_client),
        provider_can_disable,
        GlobalInputs::resource(
            global.verify,
            global.validation.as_ref(),
            global.resolved_ca_certificates.as_deref(),
            global.client_certificate_ref.is_some(),
            global.resolved_client_certificate.as_ref(),
        ),
    )
}

/// Resolve one captured owning global generation. Local owning proofs are added
/// by the respective transport migration; inherited proofs are mandatory here.
pub fn resolve_outbound_tls_with_snapshot(
    local: &OutboundTlsLocal,
    resolved_ca: Option<&[Secret]>,
    resolved_client: Option<&Secret>,
    provider_can_disable: bool,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    resolve_snapshot_inputs(
        LocalInputs::resource(local, resolved_ca, resolved_client),
        provider_can_disable,
        global,
    )
}

fn resolve_snapshot_inputs(
    local: LocalInputs<'_>,
    provider_can_disable: bool,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let proofs = global.effective_proofs_for(
        local.verify.is_some(),
        local.validation.is_some(),
        local.client_declared,
    );
    if proofs[0].is_err() || proofs[1].is_err() {
        return Err(EdgionErrorCode::OutboundTlsCaMissing);
    }
    if proofs[2].is_err() {
        return Err(EdgionErrorCode::OutboundTlsClientCertInvalid);
    }
    resolve_effective_tls(
        local,
        provider_can_disable,
        GlobalInputs::resource(
            global.verify(),
            global.validation(),
            global.ca_material(),
            global.client_reference().is_some(),
            global.client_material(),
        ),
    )
}

/// Trusted file bootstrap is a direct material input, never a resource proof.
/// Its inherited axes still prove the actual captured EGC generation.
pub(crate) fn resolve_etcd_bootstrap_tls_audited(
    material: &edgion_resources::credentials::materials::TlsMaterial,
    global: &GlobalOutboundTlsSnapshot,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    let local = LocalInputs {
        verify: None,
        validation: material.ca_pem.as_deref().map(|pem| ValidationInput {
            policy: None,
            ca: CaInput::Pem(pem),
        }),
        client_declared: false,
        client: material
            .client_cert_pem
            .as_deref()
            .zip(material.client_key_pem.as_ref())
            .map(|(certificate, private_key)| ClientInput::Pem {
                certificate,
                private_key: private_key.as_slice(),
            }),
    };
    let result = resolve_snapshot_inputs(local, false, global);
    audit_resolution(&result, global.verify(), "Etcd");
    result
}

#[derive(Clone, Copy)]
enum CaInput<'a> {
    System,
    Secrets {
        expected: usize,
        material: Option<&'a [Secret]>,
    },
    Pem(&'a [u8]),
}

#[derive(Clone, Copy)]
enum ClientInput<'a> {
    Secret(&'a Secret),
    Pem {
        certificate: &'a [u8],
        private_key: &'a [u8],
    },
}

#[derive(Clone, Copy)]
struct ValidationInput<'a> {
    policy: Option<&'a OutboundTlsValidation>,
    ca: CaInput<'a>,
}

impl<'a> ValidationInput<'a> {
    fn resource(policy: &'a OutboundTlsValidation, material: Option<&'a [Secret]>) -> Self {
        let ca = match policy.ca_certificate_refs.as_deref().filter(|refs| !refs.is_empty()) {
            Some(refs) => CaInput::Secrets {
                expected: refs.len(),
                material,
            },
            None => CaInput::System,
        };
        Self {
            policy: Some(policy),
            ca,
        }
    }
}

struct LocalInputs<'a> {
    verify: Option<bool>,
    validation: Option<ValidationInput<'a>>,
    client_declared: bool,
    client: Option<ClientInput<'a>>,
}
impl<'a> LocalInputs<'a> {
    fn resource(local: &'a OutboundTlsLocal, ca: Option<&'a [Secret]>, client: Option<&'a Secret>) -> Self {
        Self {
            verify: local.verify,
            validation: local
                .validation
                .as_ref()
                .map(|policy| ValidationInput::resource(policy, ca)),
            client_declared: local.client_certificate_ref.is_some(),
            client: client.map(ClientInput::Secret),
        }
    }
}

struct GlobalInputs<'a> {
    verify: bool,
    validation: Option<ValidationInput<'a>>,
    client_declared: bool,
    client: Option<ClientInput<'a>>,
}
impl<'a> GlobalInputs<'a> {
    fn resource(
        verify: bool,
        validation: Option<&'a OutboundTlsValidation>,
        ca: Option<&'a [Secret]>,
        client_declared: bool,
        client: Option<&'a Secret>,
    ) -> Self {
        Self {
            verify,
            validation: validation.map(|policy| ValidationInput::resource(policy, ca)),
            client_declared,
            client: client.map(ClientInput::Secret),
        }
    }
}
fn resolve_effective_tls(
    local: LocalInputs<'_>,
    provider_can_disable: bool,
    global: GlobalInputs<'_>,
) -> Result<ResolvedOutboundTls, EdgionErrorCode> {
    // Whole-block precedence: a present local `validation` shadows the global
    // block entirely (fields are not merged). CA bytes are sourced from whichever
    // block wins — the per-call `resolved_ca` for a local block, or the
    // Controller-shipped global bundles when inheriting the global block. This
    // deliberately does NOT fall back from local to global CA: the global
    // resolved bundles correspond to the global refs, not the local ones.
    let selection = local.validation.or(global.validation);
    let validation = selection.and_then(|selected| selected.policy);
    if validation.is_some_and(|value| {
        matches!(
            value.well_known_ca_certificates.as_ref(),
            Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::Other(_))
        )
    }) {
        return Err(EdgionErrorCode::OutboundTlsCaMissing);
    }
    let (client_ref_configured, resolved_client_effective) = if local.client_declared {
        (true, local.client)
    } else if global.client_declared {
        (true, global.client)
    } else {
        // Preserve direct programmatic callers that supply already-resolved
        // identity material without an operator-facing reference.
        (false, local.client)
    };

    // Fail-secure: a non-empty subjectAltNames policy forces full verification
    // even if the operator set verify=false — SAN pinning is meaningless without
    // chain verification, so it overrides the skip-verify request.
    let san_pinned = validation
        .and_then(|v| v.subject_alt_names.as_ref())
        .is_some_and(|s| !s.is_empty());
    let requested_verify = local.verify.unwrap_or(global.verify);
    let verify = requested_verify || san_pinned;

    if client_ref_configured && resolved_client_effective.is_none() {
        return Err(EdgionErrorCode::OutboundTlsClientCertInvalid);
    }

    if !verify {
        if !provider_can_disable {
            return Err(EdgionErrorCode::OutboundTlsVerifyUnsupported);
        }
        // Operator explicitly disabled verification; honored as their responsibility.
        return Ok(ResolvedOutboundTls {
            verify: false,
            ca: EffectiveCa::System,
            client_identity: None,
            san: validation.cloned(),
        });
    }

    // verify == true: resolve CA source from the winning validation block.
    let ca = match selection.map(|selected| selected.ca).unwrap_or(CaInput::System) {
        CaInput::Secrets { expected, material } => {
            // CA bytes must come from the block that won precedence above; no
            // cross-block fallback (see the tuple selection comment).
            let secrets: &[Secret] = material
                .filter(|s| !s.is_empty())
                .ok_or(EdgionErrorCode::OutboundTlsCaMissing)?;
            if secrets.len() != expected {
                return Err(EdgionErrorCode::OutboundTlsCaMissing);
            }
            let bundles: Vec<Vec<u8>> = secrets
                .iter()
                .map(|secret| {
                    secret
                        .data
                        .as_ref()
                        .and_then(|data| data.get(tls_keys::CA_CERT))
                        .map(|value| value.0.clone())
                        .ok_or(EdgionErrorCode::OutboundTlsCaMissing)
                })
                .collect::<Result<_, _>>()?;
            EffectiveCa::Custom(bundles)
        }
        // wellKnownCACertificates: System, or both absent → public roots.
        CaInput::System => EffectiveCa::System,
        CaInput::Pem(pem) => EffectiveCa::Custom(vec![pem.to_vec()]),
    };

    // Resolve optional client mTLS identity
    let client_identity = match resolved_client_effective {
        Some(ClientInput::Secret(secret)) => {
            let cert = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::CERT))
                .map(|v| v.0.clone())
                .ok_or(EdgionErrorCode::OutboundTlsClientCertInvalid)?;
            let key = secret
                .data
                .as_ref()
                .and_then(|d| d.get(tls_keys::KEY))
                .map(|v| Zeroizing::new(v.0.clone()))
                .ok_or(EdgionErrorCode::OutboundTlsClientCertInvalid)?;
            Some((cert, key))
        }
        Some(ClientInput::Pem {
            certificate,
            private_key,
        }) => Some((certificate.to_vec(), Zeroizing::new(private_key.to_vec()))),
        None => None,
    };

    Ok(ResolvedOutboundTls {
        verify,
        ca,
        client_identity,
        san: validation.cloned(),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/outbound_tls_test.rs"]
mod tests;
