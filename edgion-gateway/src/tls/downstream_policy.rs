//! One early-handshake owner for inherited downstream TLS policy.
use super::curve_policy::CurvePolicy;
use edgion_resources::resources::downstream_tls::{validate_cipher_list, DownstreamTlsConfig};
use edgion_resources::resources::edgion_tls::TlsVersion;
use std::ffi::CString;

/// An empty operator list is explicit backend defaults, not inheritance.
#[derive(Clone, Debug, Default)]
pub(crate) enum CipherPolicy {
    #[default]
    Inherit,
    BackendDefault,
    Explicit(CString),
    Reject(&'static str),
}

impl CipherPolicy {
    pub(crate) fn compile(ciphers: Option<&[String]>) -> Self {
        if let Err(reason) = validate_cipher_list(ciphers) {
            return Self::Reject(reason);
        }
        match ciphers {
            None => Self::Inherit,
            Some([]) => Self::BackendDefault,
            Some(values) => match CString::new(values.join(":")) {
                Ok(value) => Self::Explicit(value),
                Err(_) => Self::Reject("ciphers must not contain NUL bytes"),
            },
        }
    }

    fn resolve<'a>(&'a self, local: Option<&'a Self>) -> &'a Self {
        match local {
            Some(Self::Inherit) | None => self,
            Some(policy) => policy,
        }
    }

    #[cfg(feature = "boringssl")]
    pub(crate) fn apply(
        &self,
        ssl: &mut pingora_core::tls::ssl::SslRef,
        enforce_fips: bool,
    ) -> Result<(), &'static str> {
        let value = match self {
            Self::Reject(reason) => return Err(reason),
            Self::Explicit(value) => value.as_c_str(),
            Self::Inherit if enforce_fips => super::fips_profile::FIPS_TLS12_CIPHERS_C,
            Self::Inherit | Self::BackendDefault => return Ok(()),
        };
        // SAFETY: ssl is exclusively borrowed and the cold/static C string
        // outlives this call. The supported backend exposes only a string setter;
        // its internal parsing remains here, without Rust hot-path preparation.
        let result =
            unsafe { boring_sys::SSL_set_strict_cipher_list(ssl as *mut _ as *mut boring_sys::SSL, value.as_ptr()) };
        if result == 1 {
            Ok(())
        } else {
            Err("cipher config rejected")
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DownstreamTlsPolicy {
    pub(crate) curves: CurvePolicy,
    pub(crate) min_tls_version: Option<TlsVersion>,
    pub(crate) ciphers: CipherPolicy,
}

impl DownstreamTlsPolicy {
    pub(crate) fn compile(config: Option<&DownstreamTlsConfig>) -> Self {
        Self {
            curves: CurvePolicy::compile(config.map_or(Ok(None), DownstreamTlsConfig::curves)),
            min_tls_version: config.and_then(|config| config.min_tls_version),
            ciphers: CipherPolicy::compile(config.and_then(|config| config.ciphers.as_deref())),
        }
    }

    /// Call exactly once after identity selection, before version negotiation.
    /// Each axis is resolved before application: a local override must not be
    /// rejected by a default that is not effective for this connection.
    #[cfg(feature = "boringssl")]
    pub(crate) fn apply(
        &self,
        ssl: &mut pingora_core::tls::ssl::SslRef,
        local_minimum: Option<TlsVersion>,
        local_ciphers: Option<&CipherPolicy>,
        enforce_fips: bool,
    ) -> Result<(), &'static str> {
        if let Some(version) = local_minimum.or(self.min_tls_version) {
            use pingora_core::tls::ssl::SslVersion;
            let version = match version {
                TlsVersion::Tls10 => SslVersion::TLS1,
                TlsVersion::Tls11 => SslVersion::TLS1_1,
                TlsVersion::Tls12 => SslVersion::TLS1_2,
                TlsVersion::Tls13 => SslVersion::TLS1_3,
            };
            ssl.set_min_proto_version(Some(version))
                .map_err(|_| "minimum TLS version rejected")?;
        }
        self.curves.apply(ssl, enforce_fips)?;
        self.ciphers.resolve(local_ciphers).apply(ssl, enforce_fips)
    }
}

#[cfg(all(test, feature = "boringssl"))]
#[path = "../../tests/unit/tls/downstream_policy_test.rs"]
mod tests;
