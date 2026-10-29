//! Cold-compiled downstream group policy, shared by all certificate sources.
use edgion_resources::resources::downstream_tls::EcdhCurve;

#[derive(Clone, Debug, Default)]
pub(crate) enum CurvePolicy {
    #[default]
    Inherit,
    Explicit {
        groups: Vec<u16>,
        fips_compatible: bool,
    },
    Reject(&'static str),
}

impl CurvePolicy {
    pub(crate) fn compile(curves: Result<Option<Vec<EcdhCurve>>, &'static str>) -> Self {
        match curves {
            Ok(None) => Self::Inherit,
            Err(reason) => Self::Reject(reason),
            Ok(Some(curves)) => Self::Explicit {
                fips_compatible: !curves.contains(&EcdhCurve::X25519),
                groups: curves.into_iter().map(group_id).collect(),
            },
        }
    }

    #[cfg(feature = "boringssl")]
    pub(crate) fn apply(
        &self,
        ssl: &mut pingora_core::tls::ssl::SslRef,
        enforce_fips: bool,
    ) -> Result<(), &'static str> {
        match self {
            Self::Reject(reason) => Err(reason),
            Self::Explicit {
                fips_compatible: false, ..
            } if enforce_fips => Err("ECDH curves conflict with FIPS policy"),
            Self::Explicit { groups, .. } => set_groups(ssl, groups),
            Self::Inherit if enforce_fips => {
                super::fips_profile::configure_fips_curves(ssl).map_err(|_| "FIPS curves rejected")
            }
            Self::Inherit => Ok(()),
        }
    }
}

#[cfg(feature = "boringssl")]
fn group_id(curve: EcdhCurve) -> u16 {
    match curve {
        EcdhCurve::X25519 => boring_sys::SSL_GROUP_X25519 as u16,
        EcdhCurve::P256 => boring_sys::SSL_GROUP_SECP256R1 as u16,
        EcdhCurve::P384 => boring_sys::SSL_GROUP_SECP384R1 as u16,
        EcdhCurve::P521 => boring_sys::SSL_GROUP_SECP521R1 as u16,
    }
}

#[cfg(not(feature = "boringssl"))]
fn group_id(curve: EcdhCurve) -> u16 {
    // IANA supported-groups IDs; the dormant backend has no application path.
    match curve {
        EcdhCurve::X25519 => 29,
        EcdhCurve::P256 => 23,
        EcdhCurve::P384 => 24,
        EcdhCurve::P521 => 25,
    }
}

#[cfg(feature = "boringssl")]
pub(crate) fn set_groups(ssl: &mut pingora_core::tls::ssl::SslRef, groups: &[u16]) -> Result<(), &'static str> {
    // SAFETY: ssl is live and exclusively borrowed; groups remains valid for the
    // call. BoringSSL copies this bounded list into its per-connection config.
    // Numeric IDs avoid its string parser and temporary parsing allocation.
    let result =
        unsafe { boring_sys::SSL_set1_group_ids(ssl as *mut _ as *mut boring_sys::SSL, groups.as_ptr(), groups.len()) };
    if result == 1 {
        Ok(())
    } else {
        Err("ECDH curves rejected")
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tls/curve_policy_test.rs"]
mod tests;
