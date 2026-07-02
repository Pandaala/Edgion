//! FIPS-approved TLS profile (Model A+).
//!
//! When the gateway is a FIPS build, downstream listeners default to the
//! FIPS-approved cipher/curve set unless the operator overrides via
//! `EdgionTls.ciphers`. See docs/superpowers/specs/2026-07-02-boringssl-fips-design.md.
//!
//! Backend-agnostic surface: `fips_module_available` and `decide_cipher_action`
//! are callable on both the boringssl and rustls builds (the startup path uses
//! them). The BoringSSL FFI helpers that take an `SslRef` are
//! `#[cfg(feature = "boringssl")]`-gated, mirroring every other `SslRef` user in
//! this crate; their only callers live in the boringssl-gated handshake code.

#[cfg(feature = "boringssl")]
use pingora_core::tls::ssl::SslRef;
#[cfg(feature = "boringssl")]
use pingora_core::{Error as PingoraError, ErrorType};

/// FIPS-approved TLS 1.2 cipher suites (Envoy FIPS default set), colon-joined
/// for BoringSSL's `SSL_set_strict_cipher_list`. Excludes ChaCha20/CBC/3DES.
pub const FIPS_TLS12_CIPHERS: &str = "ECDHE-ECDSA-AES128-GCM-SHA256:\
ECDHE-RSA-AES128-GCM-SHA256:\
ECDHE-ECDSA-AES256-GCM-SHA384:\
ECDHE-RSA-AES256-GCM-SHA384";

/// FIPS-approved ECDH curves (NIST P-curves), for `SSL_set1_curves_list`.
pub const FIPS_CURVES: &str = "P-256:P-384:P-521";

/// Whether this binary links the FIPS-validated crypto module. Compile-time,
/// process-wide; `false` on any non-boringssl build.
#[cfg(feature = "boringssl")]
pub fn fips_module_available() -> bool {
    boring::fips::enabled()
}

#[cfg(not(feature = "boringssl"))]
pub fn fips_module_available() -> bool {
    false
}

/// Process-global override that forces the FIPS-approved TLS policy (approved
/// cipher list + curves) on downstream listeners even when the validated crypto
/// module is absent (a non-FIPS build). Default false; set once at startup from
/// gateway config / `EDGION_FIPS_POLICY`. This lets the FIPS policy be exercised
/// in integration tests on a normal boringssl build, and lets an operator opt
/// into the approved-suite restriction as defense-in-depth on a non-FIPS build.
static FIPS_POLICY_FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set the force-FIPS-policy override (startup only).
pub fn set_fips_policy_forced(enabled: bool) {
    FIPS_POLICY_FORCED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the force-FIPS-policy override is set.
pub fn fips_policy_forced() -> bool {
    FIPS_POLICY_FORCED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether downstream listeners should enforce the FIPS-approved cipher/curve
/// set for this connection: true on a real FIPS build (validated module linked),
/// OR when the policy is force-enabled via config/env. This is the single gate
/// the handshake paths consult.
pub fn fips_policy_active() -> bool {
    fips_module_available() || fips_policy_forced()
}

/// Which cipher-list configuration `apply_edgion_tls_cert` should apply
/// downstream. This governs only the TLS 1.2 cipher *list*; the FIPS-approved
/// *curve* restriction is orthogonal and, on a FIPS build, always applies
/// regardless of this decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAction {
    /// Operator set `EdgionTls.ciphers`; use them verbatim (operator override).
    Operator,
    /// FIPS build, no operator ciphers; apply the approved cipher list.
    FipsDefault,
    /// Non-FIPS build, no operator ciphers; leave BoringSSL's default.
    BoringDefault,
}

/// Decide the cipher-list action. Operator ciphers always win (the free
/// per-workload override); otherwise, when FIPS enforcement is active, apply the
/// approved list. `enforce_fips` is `fips_policy_active()` at the call site.
pub fn decide_cipher_action(has_operator_ciphers: bool, enforce_fips: bool) -> CipherAction {
    match (has_operator_ciphers, enforce_fips) {
        (true, _) => CipherAction::Operator,
        (false, true) => CipherAction::FipsDefault,
        (false, false) => CipherAction::BoringDefault,
    }
}

/// Apply the FIPS-approved TLS 1.2 cipher list to this connection. Mirrors
/// `TlsCallback::configure_ciphers`'s strict fail-closed FFI: BoringSSL rejects
/// unknown names, so a non-1 return must abort (the caller stamps `err_log` and
/// returns before binding cert). TLS 1.3 suites are not configurable in
/// BoringSSL and are intentionally left untouched.
#[cfg(feature = "boringssl")]
pub(crate) fn configure_fips_ciphers(ssl: &mut SslRef) -> Result<(), Box<PingoraError>> {
    use std::ffi::CString;

    let ciphers = CString::new(FIPS_TLS12_CIPHERS)
        .map_err(|e| PingoraError::explain(ErrorType::InternalError, format!("fips cipher null byte: {e}")))?;

    // SAFETY: SslRef -> SSL* for FFI; ssl valid for this call; CString outlives
    // the FFI call.
    unsafe {
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        if boring_sys::SSL_set_strict_cipher_list(ssl_ptr, ciphers.as_ptr()) != 1 {
            return Err(PingoraError::explain(
                ErrorType::InternalError,
                "fips default cipher list rejected".to_string(),
            ));
        }
    }
    Ok(())
}

/// Apply the FIPS-approved ECDH curve restriction to this connection. Applied
/// on any FIPS build independently of the cipher-list decision, because an
/// operator `spec.ciphers` override must not silently re-enable a non-approved
/// curve (e.g. X25519). Same strict fail-closed FFI contract as
/// `configure_fips_ciphers`.
#[cfg(feature = "boringssl")]
pub(crate) fn configure_fips_curves(ssl: &mut SslRef) -> Result<(), Box<PingoraError>> {
    use std::ffi::CString;

    let curves = CString::new(FIPS_CURVES)
        .map_err(|e| PingoraError::explain(ErrorType::InternalError, format!("fips curves null byte: {e}")))?;

    // SAFETY: SslRef -> SSL* for FFI; ssl valid for this call; CString outlives
    // the FFI call.
    unsafe {
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        if boring_sys::SSL_set1_curves_list(ssl_ptr, curves.as_ptr()) != 1 {
            return Err(PingoraError::explain(
                ErrorType::InternalError,
                "fips curves list rejected".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_cipher_action_operator_always_wins() {
        assert_eq!(decide_cipher_action(true, true), CipherAction::Operator);
        assert_eq!(decide_cipher_action(true, false), CipherAction::Operator);
    }

    #[test]
    fn decide_cipher_action_fips_default_when_no_operator_ciphers() {
        assert_eq!(decide_cipher_action(false, true), CipherAction::FipsDefault);
    }

    #[test]
    fn decide_cipher_action_boring_default_on_non_fips() {
        assert_eq!(decide_cipher_action(false, false), CipherAction::BoringDefault);
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn approved_tls12_ciphers_are_valid_boringssl_names() {
        use boring::ssl::{SslContext, SslMethod};
        let mut b = SslContext::builder(SslMethod::tls_server()).unwrap();
        // set_cipher_list parses the TLS 1.2 cipher string; a typo yields Err.
        b.set_cipher_list(FIPS_TLS12_CIPHERS)
            .expect("approved TLS 1.2 cipher names must be valid BoringSSL strings");
    }

    #[cfg(not(feature = "fips"))]
    #[test]
    fn fips_module_absent_without_feature() {
        assert!(!fips_module_available());
    }

    #[cfg(not(feature = "fips"))]
    #[test]
    fn fips_policy_forced_gates_enforcement_on_non_fips_build() {
        // On a non-FIPS build the module is absent, so policy enforcement is off
        // by default and follows the force override. Serialized to avoid racing
        // the process-global with other tests that read it (none currently do).
        assert!(!fips_module_available());
        set_fips_policy_forced(false);
        assert!(!fips_policy_active(), "off by default without the module");
        set_fips_policy_forced(true);
        assert!(fips_policy_active(), "force override enables enforcement");
        set_fips_policy_forced(false); // reset shared state
    }

    #[cfg(feature = "fips")]
    #[test]
    fn fips_module_reports_enabled() {
        assert!(fips_module_available(), "fips build must link the validated module");
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn configure_fips_ciphers_and_curves_apply_cleanly() {
        use boring::ssl::{Ssl, SslContext, SslMethod};
        let ctx = SslContext::builder(SslMethod::tls_server()).unwrap().build();
        let mut ssl = Ssl::new(&ctx).unwrap();
        // Exercises the real strict cipher FFI and the curves FFI on a live Ssl,
        // covering the "approved strings accepted, else fail closed" risk.
        configure_fips_ciphers(ssl.as_mut()).expect("approved fips ciphers must apply on a boringssl build");
        configure_fips_curves(ssl.as_mut()).expect("approved fips curves must apply on a boringssl build");
    }

    /// End-to-end: a server that applies the FIPS-approved policy
    /// (`configure_fips_ciphers` + `configure_fips_curves`) must reject a TLS 1.2
    /// client offering only ChaCha20 (non-approved) and accept an approved
    /// client. TLS 1.2 is pinned on both ends because BoringSSL cannot restrict
    /// TLS 1.3 suites (ChaCha20 stays negotiable at 1.3 — a documented gap), so
    /// the exclusion is only observable at 1.2. Runs in-process over a localhost
    /// socket on the normal boringssl build — no FIPS module / clang-12 needed.
    #[cfg(feature = "boringssl")]
    #[test]
    fn fips_policy_rejects_chacha20_and_accepts_approved_at_tls12() {
        use boring::pkey::PKey;
        use boring::ssl::{Ssl, SslContext, SslMethod, SslVerifyMode, SslVersion};
        use boring::x509::X509;
        use std::net::{TcpListener, TcpStream};
        use std::time::Duration;

        // Self-signed cert (rcgen default key is ECDSA P-256).
        let key_pair = rcgen::KeyPair::generate().expect("rcgen key");
        let params = rcgen::CertificateParams::new(vec![]).expect("rcgen params");
        let cert = params.self_signed(&key_pair).expect("rcgen self-sign");
        let cert_pem = cert.pem().into_bytes();
        let key_pem = key_pair.serialize_pem().into_bytes();

        // Run one TLS 1.2 handshake with the given client cipher list; return
        // whether the FIPS-policy server accepted it (server side is authoritative).
        // TLS version + client cipher list are set on the context builders
        // (boring exposes set_cipher_list only on SslContextBuilder); the code
        // under test — configure_fips_ciphers/curves — runs on the server's
        // per-connection SslRef, exactly as in the handshake callback.
        let run = |client_ciphers: &str| -> bool {
            let x509 = X509::from_pem(&cert_pem).unwrap();
            let pkey = PKey::private_key_from_pem(&key_pem).unwrap();

            let mut sb = SslContext::builder(SslMethod::tls_server()).unwrap();
            sb.set_certificate(&x509).unwrap();
            sb.set_private_key(&pkey).unwrap();
            sb.set_min_proto_version(Some(SslVersion::TLS1_2)).unwrap();
            sb.set_max_proto_version(Some(SslVersion::TLS1_2)).unwrap();
            let server_ctx = sb.build();

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();

            let server = std::thread::spawn(move || {
                let (tcp, _) = listener.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
                tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();
                let mut ssl = Ssl::new(&server_ctx).unwrap();
                // The code under test: apply the FIPS-approved policy.
                configure_fips_ciphers(ssl.as_mut()).unwrap();
                configure_fips_curves(ssl.as_mut()).unwrap();
                ssl.accept(tcp).is_ok()
            });

            let tcp = TcpStream::connect(addr).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
            tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut cb = SslContext::builder(SslMethod::tls_client()).unwrap();
            cb.set_verify(SslVerifyMode::NONE);
            cb.set_min_proto_version(Some(SslVersion::TLS1_2)).unwrap();
            cb.set_max_proto_version(Some(SslVersion::TLS1_2)).unwrap();
            cb.set_cipher_list(client_ciphers).unwrap();
            let client_ctx = cb.build();
            let cssl = Ssl::new(&client_ctx).unwrap();
            let _ = cssl.connect(tcp); // client result ignored; assert on server

            server.join().unwrap()
        };

        assert!(
            !run("ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-CHACHA20-POLY1305"),
            "FIPS-policy server must reject a ChaCha20-only TLS 1.2 client"
        );
        assert!(
            run(FIPS_TLS12_CIPHERS),
            "FIPS-policy server must accept an approved-suite TLS 1.2 client"
        );
    }
}
