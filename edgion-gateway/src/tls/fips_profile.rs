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
#[path = "../../tests/unit/tls/fips_profile_test.rs"]
mod tests;
