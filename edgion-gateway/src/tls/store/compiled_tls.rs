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
use crate::runtime::matching::tls::CompiledServingCertificate;
use edgion_resources::resources::edgion_tls::{ClientAuthMode, EdgionTls};
use pingora_core::tls::pkey::PKey;
use pingora_core::tls::x509::store::{X509Store, X509StoreBuilder};
use pingora_core::tls::x509::X509;
use std::ffi::CString;
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
    /// Cold-compiled operator cipher list. `Ok(None)` means `spec.ciphers` is
    /// absent or empty, which the handshake treats as "leave the inherited
    /// suite in place" exactly as before.
    pub operator_cipher: Result<Option<CString>, String>,
}

impl CompiledEdgionTls {
    /// Compile the handshake-ready state for a resource that will reach the
    /// matcher.
    pub fn compile(tls: Arc<EdgionTls>) -> Self {
        Self {
            identity: compile_identity(&tls),
            client_ca_store: compile_client_ca_store(&tls),
            operator_cipher: compile_operator_cipher(&tls),
            tls,
        }
    }

    /// Wrap a resource that is not matchable, without compiling secret-bearing
    /// state it can never use.
    pub fn not_compiled(tls: Arc<EdgionTls>) -> Self {
        Self {
            tls,
            identity: Err(NOT_COMPILED),
            client_ca_store: Ok(None),
            operator_cipher: Ok(None),
        }
    }
}

/// Compile the complete ordered serving chain and its private key.
///
/// The diagnostic categories are the ones the handshake path used while it
/// parsed inline, so tls.log vocabulary is unchanged. A CRYPTO_BUFFER
/// allocation failure folds into `parse certificate failed`, matching the
/// Gateway Listener path, which collapses the same failure into its generic
/// compile diagnostic.
fn compile_identity(tls: &EdgionTls) -> Result<Arc<CompiledServingCertificate>, &'static str> {
    let cert_pem = tls.cert_pem().map_err(|_| "read certificate failed")?;
    let strict_chain = edgion_common::tls_validation::parse_strict_certificate_chain(cert_pem.as_bytes())
        .map_err(|_| "parse certificate failed")?;
    #[cfg(feature = "boringssl")]
    let atomic_chain = Arc::new(
        CompiledBoringCertificateChain::from_der(&strict_chain.certificates_der).ok_or("parse certificate failed")?,
    );
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
        #[cfg(feature = "boringssl")]
        atomic_chain,
        key_type: private_key.id(),
        private_key,
        validity: strict_chain.validity,
    }))
}

/// Build the mTLS client trust store.
///
/// `validate_cert` only checks that a CA reference is present for
/// Mutual/OptionalMutual; it never inspects the controller-resolved `ca.crt`
/// content. This is therefore the one compile step that genuinely fails for
/// resources the store accepted as valid, which is why the failure is carried
/// to the handshake instead of removing the resource from the matcher.
///
/// The error strings are the exact messages `configure_mtls` produced before,
/// so the `mtls config: …` audit text is unchanged.
fn compile_client_ca_store(tls: &EdgionTls) -> Result<Option<Arc<X509Store>>, String> {
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

    let ca_pem = tls
        .ca_cert_pem()
        .map_err(|e| format!("Failed to get CA cert PEM for mTLS: {}", e))?;
    let mut store_builder = X509StoreBuilder::new().map_err(|e| format!("Failed to create X509 store: {}", e))?;
    add_ca_bundle_to_store(&mut store_builder, ca_pem.as_bytes())?;
    Ok(Some(Arc::new(store_builder.build())))
}

/// Parse a (possibly multi-block) CA bundle PEM and add every certificate to
/// the store, returning the number added. RFC 5280 §6.1 path validation needs
/// the complete intermediate chain up to a trust anchor, so all PEM blocks must
/// be loaded — `X509::from_pem` reads only the first block. An empty or
/// unparseable bundle is rejected (fail-closed) so a misconfigured CA secret
/// cannot silently produce an empty trust store.
fn add_ca_bundle_to_store(store_builder: &mut X509StoreBuilder, ca_pem: &[u8]) -> Result<usize, String> {
    let bundle = edgion_common::tls_validation::parse_canonical_ca_bundle(ca_pem)
        .map_err(|error| format!("Invalid CA bundle: {error:?}"))?;
    let count = bundle.certificates_der.len();
    for der in bundle.certificates_der {
        let cert = X509::from_der(&der).map_err(|error| format!("Invalid CA DER: {error}"))?;
        store_builder
            .add_cert(cert)
            .map_err(|e| format!("Failed to add CA cert to store: {}", e))?;
    }
    Ok(count)
}

/// Render `spec.ciphers` into the NUL-terminated form
/// `SSL_set_strict_cipher_list` consumes. The FFI call itself stays on the
/// handshake path so its fail-closed rejection behaviour (tls-F002) is
/// unchanged; only the join and the `CString` allocation move here.
fn compile_operator_cipher(tls: &EdgionTls) -> Result<Option<CString>, String> {
    let Some(ciphers) = tls.spec.ciphers.as_ref() else {
        return Ok(None);
    };
    if ciphers.is_empty() {
        return Ok(None);
    }
    CString::new(ciphers.join(":"))
        .map(Some)
        .map_err(|e| format!("cipher null byte: {e}"))
}

#[cfg(all(test, feature = "boringssl"))]
#[path = "../../../tests/unit/tls/store/compiled_tls_test.rs"]
mod tests;
