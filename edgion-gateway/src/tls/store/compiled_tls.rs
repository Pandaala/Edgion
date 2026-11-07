//! Cold-compiled EdgionTls runtime state.
//!
//! Every field here is compiled once, while an accepted `TlsStore` generation is
//! published, and is then immutable for the life of that generation. The TLS
//! handshake path consumes these values directly: it performs no Secret byte
//! copies, no PEM parsing, no ASN.1 decoding, and no trust-store construction.
//!
//! Compilation is deliberately fallible *without* removing the resource from the
//! matcher. A resource that fails to compile keeps its hostname slot and its
//! collision precedence, and fails the handshake closed with the same static
//! diagnostic category the handshake produced when it still parsed inline.
//! Dropping it from the index instead would silently promote the next candidate
//! for that hostname, which is a security-relevant behavior change.
//!
//! Nothing in this module derives `Debug`, `Clone`, or `serde` traits: it owns
//! private-key material and must never reach logs, admin output, or conf_sync.

#[cfg(feature = "boringssl")]
use crate::runtime::matching::tls::CompiledBoringCertificateChain;
use crate::runtime::matching::tls::{
    der_list_digest, CompiledServingCertificate, SessionContextId, SESSION_CONTEXT_KIND_EDGION_TLS,
};
use crate::tls::downstream_policy::CipherPolicy;
use edgion_resources::resources::allowed_san::{AllowedSanMatch, AllowedSanType};
use edgion_resources::resources::edgion_tls::{ClientAuthMode, EdgionTls};
use pingora_core::tls::pkey::PKey;
use pingora_core::tls::x509::store::{X509Store, X509StoreBuilder};
use pingora_core::tls::x509::X509;
use std::sync::Arc;

/// Placeholder category for an entry that never reaches the matcher
/// (`matchable == false`), so its identity was intentionally not compiled.
/// Unreachable by construction; kept honest rather than borrowing a category
/// that would misdirect an operator if a future path ever consumed it.
const NOT_COMPILED: &str = "certificate not compiled";

/// One accepted EdgionTls generation plus everything the handshake needs from it
/// in already-usable form.
pub struct CompiledEdgionTls {
    /// The accepted resource generation. Precedence ordering, collision logging
    /// and the mTLS allowlist callback continue to key on this `Arc`, so all
    /// existing `Arc::ptr_eq` identity checks keep their meaning.
    pub tls: Arc<EdgionTls>,
    /// Cold-compiled serving identity, or the static diagnostic category the
    /// handshake must stamp instead.
    pub identity: Result<Arc<CompiledServingCertificate>, &'static str>,
    /// Cold-compiled client trust store. `Ok(None)` means no store is needed:
    /// either no `clientAuth`, or `ClientAuthMode::Terminate`, which presents a
    /// server certificate but never requests a client certificate.
    pub client_ca_store: Result<Option<Arc<X509Store>>, String>,
    /// Shared cold cipher preparation; absence and explicit backend defaults
    /// remain distinct until the selected identity inherits its EGC policy.
    pub(crate) operator_cipher: CipherPolicy,
    /// Session id context of this generation's effective policy.
    pub(crate) session_context: SessionContextId,
}

impl CompiledEdgionTls {
    /// Compile the handshake-ready state for a resource that will reach the
    /// matcher.
    pub fn compile(tls: Arc<EdgionTls>) -> Self {
        let identity = compile_identity(&tls);
        let client_ca = compile_client_ca_store(&tls);
        let session_context = edgion_tls_session_context(
            &tls,
            identity.as_ref().ok().map(|identity| &identity.chain_digest),
            client_ca
                .as_ref()
                .ok()
                .and_then(|ca| ca.as_ref())
                .map(|(_, digest)| digest),
        );
        Self {
            identity,
            client_ca_store: client_ca.map(|ca| ca.map(|(store, _)| store)),
            operator_cipher: CipherPolicy::compile(tls.spec.ciphers.as_deref()),
            session_context,
            tls,
        }
    }

    /// Wrap a resource that is not matchable, without compiling secret-bearing
    /// state it can never use.
    pub fn not_compiled(tls: Arc<EdgionTls>) -> Self {
        Self {
            session_context: edgion_tls_session_context(&tls, None, None),
            tls,
            identity: Err(NOT_COMPILED),
            client_ca_store: Ok(None),
            operator_cipher: CipherPolicy::Inherit,
        }
    }
}

/// Derive the session id context from the effective EdgionTls policy.
///
/// Contract: every client-authentication and serving-identity input the runtime
/// applies is encoded; inputs the runtime ignores are not. Protocol version and
/// cipher are left out because BoringSSL requires both to match on resumption. A missing serving or CA digest means compilation failed,
/// and the handshake then fails closed in cert_cb, so no session can exist
/// under that marker. When the EdgionTls is selected on its own, Terminate and
/// absent client auth behave identically (no CertificateRequest,
/// `configure_mtls` returns early), as do empty and absent allow-lists (the
/// verify callback skips empty lists). An overlay rejects any `clientAuth`, so
/// a Terminate overlay never issues a session to share.
fn edgion_tls_session_context(
    tls: &EdgionTls,
    chain_digest: Option<&[u8; 32]>,
    ca_digest: Option<&[u8; 32]>,
) -> SessionContextId {
    let mut hasher = SessionContextId::hasher(SESSION_CONTEXT_KIND_EDGION_TLS);
    hasher.opt_str(tls.metadata.namespace.as_deref());
    hasher.opt_str(tls.metadata.name.as_deref());
    match chain_digest {
        Some(digest) => {
            hasher.tag(1);
            hasher.bytes(digest);
        }
        None => hasher.tag(0),
    }
    let client_auth = tls.spec.client_auth.as_ref();
    match client_auth.map(|client_auth| &client_auth.mode) {
        None | Some(ClientAuthMode::Terminate) => hasher.tag(0),
        Some(ClientAuthMode::Mutual) => hasher.tag(1),
        Some(ClientAuthMode::OptionalMutual) => hasher.tag(2),
    }
    let Some(client_auth) = client_auth.filter(|client_auth| client_auth.mode != ClientAuthMode::Terminate) else {
        return SessionContextId::from_digest(hasher.finish());
    };
    hasher.u64(u64::from(client_auth.verify_depth));
    match ca_digest {
        Some(digest) => {
            hasher.tag(1);
            hasher.bytes(digest);
        }
        None => hasher.tag(0),
    }
    let sans = client_auth.allowed_sans.as_deref().unwrap_or_default();
    hasher.u64(sans.len() as u64);
    for san in sans {
        hasher.tag(match san.san_type {
            AllowedSanType::Dns => 0,
            AllowedSanType::Uri => 1,
            AllowedSanType::Email => 2,
            AllowedSanType::Ip => 3,
            AllowedSanType::OtherName => 4,
        });
        hasher.tag(match san.match_kind {
            AllowedSanMatch::Exact => 0,
            AllowedSanMatch::Prefix => 1,
            AllowedSanMatch::Suffix => 2,
            AllowedSanMatch::Contains => 3,
            AllowedSanMatch::RegularExpression => 4,
        });
        hasher.bytes(san.value.as_bytes());
        hasher.tag(u8::from(san.ignore_case));
        hasher.opt_str(san.oid.as_deref());
    }
    let cns = client_auth.allowed_cns.as_deref().unwrap_or_default();
    hasher.u64(cns.len() as u64);
    for cn in cns {
        hasher.bytes(cn.as_bytes());
    }
    SessionContextId::from_digest(hasher.finish())
}

/// Compile the complete ordered serving chain and its private key.
///
/// The diagnostic categories are the ones the handshake path used while it
/// parsed inline, so tls.log vocabulary is unchanged. A CRYPTO_BUFFER
/// allocation failure folds into `parse certificate failed`, matching the
/// Gateway Listener path, which collapses the same failure into its generic
/// compile diagnostic.
fn compile_identity(tls: &EdgionTls) -> Result<Arc<CompiledServingCertificate>, &'static str> {
    edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs::from_tls(tls)
        .server()
        .material()
        .map_err(|_| "read certificate failed")?;
    let cert_pem = tls.cert_pem().map_err(|_| "read certificate failed")?;
    let strict_chain = edgion_common::tls_validation::parse_strict_certificate_chain(cert_pem.as_bytes())
        .map_err(|_| "parse certificate failed")?;
    #[cfg(feature = "boringssl")]
    let atomic_chain = Arc::new(
        CompiledBoringCertificateChain::from_der(&strict_chain.certificates_der).ok_or("parse certificate failed")?,
    );
    let chain_digest = der_list_digest(strict_chain.certificates_der.iter().map(Vec::as_slice));
    let chain = strict_chain
        .certificates_der
        .iter()
        .map(|der| X509::from_der(der).ok())
        .collect::<Option<Vec<_>>>()
        .ok_or("parse certificate failed")?;

    let key_pem = tls.key_pem().map_err(|_| "read private key failed")?;
    let private_key = PKey::private_key_from_pem(key_pem.as_bytes()).map_err(|_| "parse private key failed")?;

    Ok(Arc::new(CompiledServingCertificate {
        chain: Arc::from(chain),
        chain_digest,
        #[cfg(feature = "boringssl")]
        atomic_chain,
        key_type: private_key.id(),
        private_key,
        validity: strict_chain.validity,
    }))
}

/// A compiled client trust store and the digest of the CA certificates it holds.
type CompiledClientCa = (Arc<X509Store>, [u8; 32]);

/// Build the mTLS client trust store.
///
/// `validate_cert` only checks that a CA reference is present for
/// Mutual/OptionalMutual; it never inspects the controller-resolved `ca.crt`
/// content. This is therefore the one compile step that genuinely fails for
/// resources the store accepted as valid, which is why the failure is carried
/// to the handshake instead of removing the resource from the matcher.
///
/// The error strings are the exact messages `configure_mtls` produced before,
/// so the `mtls config: …` audit text is unchanged. The store is returned with
/// the digest of the CA certificates it holds, for the session id context.
fn compile_client_ca_store(tls: &EdgionTls) -> Result<Option<CompiledClientCa>, String> {
    let Some(client_auth) = tls.spec.client_auth.as_ref() else {
        return Ok(None);
    };
    match client_auth.mode {
        // Terminate presents the server certificate but never requests or
        // verifies a client certificate, so it needs no client CA store. This
        // mirrors the early return `configure_mtls` still performs, and keeps a
        // valid Terminate config with no caSecretRef compiling cleanly.
        ClientAuthMode::Terminate => return Ok(None),
        ClientAuthMode::Mutual | ClientAuthMode::OptionalMutual => {}
    }

    let fields = edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs::from_tls(tls);
    let field = fields
        .client_ca()
        .ok_or_else(|| "Invalid mTLS CA Secret reference".to_string())?;
    field
        .material()
        .map_err(|_| "Invalid mTLS CA Secret reference or material identity".to_string())?;
    let ca_pem = tls
        .ca_cert_pem()
        .map_err(|e| format!("Failed to get CA cert PEM for mTLS: {}", e))?;
    let mut store_builder = X509StoreBuilder::new().map_err(|e| format!("Failed to create X509 store: {}", e))?;
    let ca_digest = add_ca_bundle_to_store(&mut store_builder, ca_pem.as_bytes())?;
    Ok(Some((Arc::new(store_builder.build()), ca_digest)))
}

/// Parse a (possibly multi-block) CA bundle PEM and add every certificate to
/// the store, returning the digest of the added DERs. RFC 5280 §6.1 path validation needs
/// the complete intermediate chain up to a trust anchor, so all PEM blocks must
/// be loaded — `X509::from_pem` reads only the first block. An empty or
/// unparseable bundle is rejected (fail-closed) so a misconfigured CA secret
/// cannot silently produce an empty trust store.
fn add_ca_bundle_to_store(store_builder: &mut X509StoreBuilder, ca_pem: &[u8]) -> Result<[u8; 32], String> {
    let bundle = edgion_common::tls_validation::parse_canonical_ca_bundle(ca_pem)
        .map_err(|error| format!("Invalid CA bundle: {error:?}"))?;
    let digest = der_list_digest(bundle.certificates_der.iter().map(Vec::as_slice));
    for der in bundle.certificates_der {
        let cert = X509::from_der(&der).map_err(|error| format!("Invalid CA DER: {error}"))?;
        store_builder
            .add_cert(cert)
            .map_err(|e| format!("Failed to add CA cert to store: {}", e))?;
    }
    Ok(digest)
}

#[cfg(all(test, feature = "boringssl"))]
#[path = "../../../tests/unit/tls/store/compiled_tls_test.rs"]
mod tests;
