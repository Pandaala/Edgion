//! Shared downstream TLS defaults; no TLS-backend dependency.
use super::edgion_tls::TlsVersion;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_ECDH_CURVES: usize = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DownstreamTlsConfig {
    /// Default minimum TLS version. EdgionTls may override it per identity.
    /// TLS1_0 and TLS1_1 are deprecated opt-ins; prefer TLS1_2 or TLS1_3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tls_version: Option<TlsVersion>,
    /// Default OpenSSL cipher expressions for TLS 1.2 and earlier.
    /// Absence inherits the FIPS/backend default; an empty list explicitly
    /// selects backend defaults. EdgionTls may replace this list per identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphers: Option<Vec<String>>,
    /// Ordered classical ECDH groups. Absence inherits the TLS backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 4), inner(length(max = 6)))]
    pub ecdh_curves: Option<Vec<String>>,
}

/// Validated vocabulary, converted to backend IDs only by the Gateway.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EcdhCurve {
    X25519,
    P256,
    P384,
    P521,
}

pub fn parse_ecdh_curves<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Vec<EcdhCurve>, &'static str> {
    let mut result = Vec::with_capacity(MAX_ECDH_CURVES);
    for name in names {
        if result.len() == MAX_ECDH_CURVES {
            return Err("ecdhCurves supports at most four curves");
        }
        let curve = match name {
            "X25519" => EcdhCurve::X25519,
            "P-256" => EcdhCurve::P256,
            "P-384" => EcdhCurve::P384,
            "P-521" => EcdhCurve::P521,
            _ => return Err("ecdhCurves contains an unsupported curve"),
        };
        if result.contains(&curve) {
            return Err("ecdhCurves contains a duplicate curve");
        }
        result.push(curve);
    }
    if result.is_empty() {
        return Err("ecdhCurves must not be empty");
    }
    Ok(result)
}

/// Validate transport-safe cipher expressions without pretending to implement
/// the backend cipher grammar. An explicit empty list is a valid override.
pub fn validate_cipher_list(ciphers: Option<&[String]>) -> Result<(), &'static str> {
    if ciphers.is_some_and(|values| values.iter().any(|value| value.contains('\0'))) {
        Err("ciphers must not contain NUL bytes")
    } else {
        Ok(())
    }
}

impl DownstreamTlsConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.curves()?;
        validate_cipher_list(self.ciphers.as_deref())
    }

    pub fn curves(&self) -> Result<Option<Vec<EcdhCurve>>, &'static str> {
        self.ecdh_curves
            .as_ref()
            .map(|names| parse_ecdh_curves(names.iter().map(String::as_str)))
            .transpose()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/downstream_tls_test.rs"]
mod tests;
