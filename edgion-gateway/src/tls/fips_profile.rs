//! FIPS-approved TLS profile (Model A+).
//!
//! When the gateway is a FIPS build, downstream listeners default to the
//! FIPS-approved cipher/curve set unless the operator overrides via
//! `EdgionTls.ciphers`. See
//! skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md.
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
#[cfg(feature = "boringssl")]
use std::ffi::CStr;

/// FIPS-approved TLS 1.2 cipher suites (Envoy FIPS default set), colon-joined
/// for BoringSSL's `SSL_set_strict_cipher_list`. Excludes ChaCha20/CBC/3DES.
pub const FIPS_TLS12_CIPHERS: &str = "ECDHE-ECDSA-AES128-GCM-SHA256:\
ECDHE-RSA-AES128-GCM-SHA256:\
ECDHE-ECDSA-AES256-GCM-SHA384:\
ECDHE-RSA-AES256-GCM-SHA384";

/// FIPS-approved ECDH curves (NIST P-curves), for `SSL_set1_curves_list`.
pub const FIPS_CURVES: &str = "P-256:P-384:P-521";

#[cfg(feature = "boringssl")]
const FIPS_TLS12_CIPHERS_C: &CStr =
    c"ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384";
#[cfg(feature = "boringssl")]
const FIPS_CURVES_C: &CStr = c"P-256:P-384:P-521";

/// Whether BoringSSL reports that this binary is running in FIPS mode.
/// Compile-time, process-wide; `false` on any non-boringssl build. This signal
/// does not establish CMVP certificate scope or build provenance.
#[cfg(feature = "boringssl")]
pub fn fips_module_available() -> bool {
    boring::fips::enabled()
}

#[cfg(not(feature = "boringssl"))]
pub fn fips_module_available() -> bool {
    false
}

/// Process-global override that forces the FIPS-approved TLS policy (approved
/// cipher list + curves) on downstream listeners even when BoringSSL FIPS mode
/// is absent (a non-FIPS build). Default false; set once at startup from
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
/// set for this connection: true on a BoringSSL FIPS-mode build,
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
    // SAFETY: SslRef -> SSL* for FFI; ssl and the static C string are valid for
    // this call.
    unsafe {
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        if boring_sys::SSL_set_strict_cipher_list(ssl_ptr, FIPS_TLS12_CIPHERS_C.as_ptr()) != 1 {
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
    // SAFETY: SslRef -> SSL* for FFI; ssl and the static C string are valid for
    // this call.
    unsafe {
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        if boring_sys::SSL_set1_curves_list(ssl_ptr, FIPS_CURVES_C.as_ptr()) != 1 {
            return Err(PingoraError::explain(
                ErrorType::InternalError,
                "fips curves list rejected".to_string(),
            ));
        }
    }
    Ok(())
}

/// How the approved cipher/curve policy is selected when BoringSSL FIPS mode is
/// absent. `Auto` follows the host kernel's FIPS mode.
///
/// This governs only the `fips_policy_forced` half of `fips_policy_active()`.
/// A genuine FIPS build enforces the profile unconditionally, so `Off` can
/// never disable enforcement there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum FipsPolicyMode {
    /// Follow the host: enforce iff the host kernel reports FIPS mode.
    #[default]
    Auto,
    /// Always enforce the approved profile.
    On,
    /// Never enforce it (on a non-FIPS build).
    Off,
}

impl FipsPolicyMode {
    /// The canonical spelling, used by `Serialize` and by log fields.
    pub fn as_str(self) -> &'static str {
        match self {
            FipsPolicyMode::Auto => "auto",
            FipsPolicyMode::On => "on",
            FipsPolicyMode::Off => "off",
        }
    }

    /// Parse one operator-supplied token. Accepts only the tri-state names.
    /// Case-insensitive, trimmed.
    pub fn parse_token(token: &str) -> Option<FipsPolicyMode> {
        match token.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(FipsPolicyMode::Auto),
            "on" => Some(FipsPolicyMode::On),
            "off" => Some(FipsPolicyMode::Off),
            _ => None,
        }
    }
}

impl serde::Serialize for FipsPolicyMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for FipsPolicyMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModeVisitor;

        impl serde::de::Visitor<'_> for ModeVisitor {
            type Value = FipsPolicyMode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(r#"one of "auto", "on", "off""#)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                FipsPolicyMode::parse_token(v).ok_or_else(|| E::custom(format!("invalid fips_policy value: {v:?}")))
            }
        }

        deserializer.deserialize_str(ModeVisitor)
    }
}

/// Whether the approved profile should be force-enabled given the operator's
/// mode and the detected host state. BoringSSL FIPS mode is NOT considered
/// here — `fips_policy_active()` OR-s it in — so that `Off` cannot disable
/// enforcement on a genuine FIPS build.
pub fn resolve_policy(mode: FipsPolicyMode, host: crate::tls::fips_host::HostFipsState) -> bool {
    match mode {
        FipsPolicyMode::On => true,
        FipsPolicyMode::Off => false,
        FipsPolicyMode::Auto => matches!(host, crate::tls::fips_host::HostFipsState::Enabled),
    }
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
        let mut b = SslContext::builder(SslMethod::tls()).unwrap();
        // set_cipher_list parses the TLS 1.2 cipher string; a typo yields Err.
        b.set_cipher_list(FIPS_TLS12_CIPHERS)
            .expect("approved TLS 1.2 cipher names must be valid BoringSSL strings");
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn ffi_profile_strings_match_the_public_profile() {
        assert_eq!(FIPS_TLS12_CIPHERS_C.to_str().unwrap(), FIPS_TLS12_CIPHERS);
        assert_eq!(FIPS_CURVES_C.to_str().unwrap(), FIPS_CURVES);
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
        assert!(fips_module_available(), "fips build must enable BoringSSL FIPS mode");
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn configure_fips_ciphers_and_curves_apply_cleanly() {
        use boring::ssl::{Ssl, SslContext, SslMethod};
        let ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
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

            let mut sb = SslContext::builder(SslMethod::tls()).unwrap();
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
            let mut cb = SslContext::builder(SslMethod::tls()).unwrap();
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

    use crate::tls::fips_host::HostFipsState;

    #[test]
    fn resolve_policy_on_and_off_ignore_the_host() {
        for host in [HostFipsState::Enabled, HostFipsState::Disabled, HostFipsState::Unknown] {
            assert!(resolve_policy(FipsPolicyMode::On, host));
            assert!(!resolve_policy(FipsPolicyMode::Off, host));
        }
    }

    #[test]
    fn resolve_policy_auto_follows_the_host_and_unknown_does_not_tighten() {
        assert!(resolve_policy(FipsPolicyMode::Auto, HostFipsState::Enabled));
        assert!(!resolve_policy(FipsPolicyMode::Auto, HostFipsState::Disabled));
        // An undetermined host must not silently restrict cipher suites.
        assert!(!resolve_policy(FipsPolicyMode::Auto, HostFipsState::Unknown));
    }

    #[test]
    fn policy_mode_deserializes_tristate_strings() {
        // Bare unquoted `on`/`off` resolve to YAML 1.1 booleans, so they must be
        // quoted here to reach the string parser at all.
        let auto: FipsPolicyMode = serde_yaml::from_str("auto").unwrap();
        assert_eq!(auto, FipsPolicyMode::Auto);
        let on_str: FipsPolicyMode = serde_yaml::from_str("\"on\"").unwrap();
        assert_eq!(on_str, FipsPolicyMode::On);
        let off_str: FipsPolicyMode = serde_yaml::from_str("\"OFF\"").unwrap();
        assert_eq!(off_str, FipsPolicyMode::Off);

        // A YAML boolean is no longer accepted (no compatibility layer).
        assert!(serde_yaml::from_str::<FipsPolicyMode>("true").is_err());
        // An unrecognized token is still rejected.
        assert!(serde_yaml::from_str::<FipsPolicyMode>("nonsense").is_err());
    }

    #[test]
    fn policy_mode_serialize_round_trips() {
        // The struct-level `#[derive(Serialize)]` on GatewayConfig forces a
        // Serialize impl. Assert on the round-trip, not the exact bytes: YAML
        // 1.1 treats `on`/`off` as booleans, so serde_yaml's emitter may quote
        // them (`'on'`). Either spelling must deserialize back to the same mode.
        for mode in [FipsPolicyMode::Auto, FipsPolicyMode::On, FipsPolicyMode::Off] {
            let yaml = serde_yaml::to_string(&mode).unwrap();
            let back: FipsPolicyMode = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(back, mode, "round-trip failed for {yaml:?}");
        }
    }

    #[test]
    fn policy_mode_default_is_auto() {
        assert_eq!(FipsPolicyMode::default(), FipsPolicyMode::Auto);
    }

    /// The full decision chain from a simulated host flag to the enforcement
    /// bool, including the production gate on the path override. Mutates the
    /// process env, so it must not run concurrently with another env test in
    /// this module; none exists today.
    #[test]
    fn simulated_fips_host_drives_auto_policy_and_gate_refuses_override() {
        use crate::tls::fips_host::{detect_host_fips, HostFipsState, FIPS_HOST_PATH_ENV};

        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("fips_enabled");
        std::fs::write(&flag, "1\n").unwrap();

        // The workspace is edition 2021, where `set_var` is still a safe fn — do
        // not wrap it in `unsafe`, that trips the `unused_unsafe` lint.
        std::env::set_var(FIPS_HOST_PATH_ENV, &flag);

        // Integration-testing mode on: the override is honored.
        let host = detect_host_fips(true);
        assert_eq!(host, HostFipsState::Enabled);
        assert!(
            resolve_policy(FipsPolicyMode::Auto, host),
            "auto must follow a FIPS host"
        );
        assert!(
            !resolve_policy(FipsPolicyMode::Off, host),
            "off must still win over the host"
        );

        // Integration-testing mode off: the override is refused and the real
        // kernel path is read instead. On any CI host that is not itself in FIPS
        // mode, that yields Disabled (the file is absent) — never Enabled from
        // our temp file.
        assert_ne!(
            detect_host_fips(false),
            HostFipsState::Enabled,
            "the path override must be refused outside integration-testing mode"
        );

        std::env::remove_var(FIPS_HOST_PATH_ENV);
    }
}
