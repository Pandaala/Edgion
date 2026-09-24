//! Pure SPIFFE URI-SAN parsing and matching for the federation plane.
//! Used by the Controller federation client to verify the Center peer
//! identity. No I/O — operates on DER bytes the caller already holds.

use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME;
use x509_parser::prelude::FromDer;

/// Upper bound on the leaf cert DER size we will parse (defensive; the cert
/// is already CA-verified by the handshake before we reach here).
const MAX_LEAF_DER_LEN: usize = 16 * 1024;
/// Upper bound on SAN entries we will enumerate.
const MAX_SAN_ENTRIES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerIdentityError {
    ParseError,
    NoSpiffeSan,
    MultiSan,
    Mismatch,
}

pub struct ControllerSpiffe {
    pub host: String,
    pub cluster: String,
    pub name: String,
}

/// Extract exactly one `spiffe://` URI SAN from a DER-encoded leaf cert.
/// Scheme comparison is case-insensitive; non-spiffe URIs and DNS/IP/email
/// SANs are ignored (they neither count nor cause rejection by presence).
pub fn extract_single_spiffe_uri(der: &[u8]) -> Result<String, PeerIdentityError> {
    if der.len() > MAX_LEAF_DER_LEN {
        return Err(PeerIdentityError::ParseError);
    }
    let (_, cert) = X509Certificate::from_der(der).map_err(|_| PeerIdentityError::ParseError)?;
    let mut spiffe: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for ext in cert.extensions() {
        if ext.oid != OID_X509_EXT_SUBJECT_ALT_NAME {
            continue;
        }
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for gn in &san.general_names {
                seen += 1;
                if seen > MAX_SAN_ENTRIES {
                    return Err(PeerIdentityError::ParseError);
                }
                if let GeneralName::URI(uri) = gn {
                    // `get(..9)` is panic-safe on non-char-boundary input; a
                    // match means the first 9 bytes are ASCII "spiffe://".
                    if uri.get(..9).is_some_and(|p| p.eq_ignore_ascii_case("spiffe://")) {
                        spiffe.push((*uri).to_string());
                    }
                }
            }
        }
    }
    match spiffe.len() {
        0 => Err(PeerIdentityError::NoSpiffeSan),
        1 => Ok(spiffe.pop().expect("len==1")),
        _ => Err(PeerIdentityError::MultiSan),
    }
}

/// Parse `spiffe://<host>/controllers/<cluster>/<name>` with strict rules:
/// exactly 3 path segments, first literal `controllers`, every segment
/// non-empty, NO normalization (no `//` collapse, no trailing-slash strip,
/// no `.`/`..` resolution, no percent-decode). Returns `None` on any deviation.
pub fn parse_controller_spiffe(uri: &str) -> Option<ControllerSpiffe> {
    // `get(..9)` is panic-safe even if byte 9 is not a char boundary; a match
    // guarantees the first 9 bytes are ASCII "spiffe://", so `&uri[9..]` is safe.
    let rest = if uri.get(..9).is_some_and(|p| p.eq_ignore_ascii_case("spiffe://")) {
        &uri[9..]
    } else {
        return None;
    };
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    let path = rest[slash..].strip_prefix('/')?; // "controllers/<cluster>/<name>"
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() != 3 || segs[0] != "controllers" {
        return None;
    }
    let (cluster, name) = (segs[1], segs[2]);
    if host.is_empty() || cluster.is_empty() || name.is_empty() {
        return None;
    }
    Some(ControllerSpiffe {
        host: host.to_string(),
        cluster: cluster.to_string(),
        name: name.to_string(),
    })
}

/// Controller-side path-only check (trust_domain verified by Center).
pub fn path_matches(parsed: &ControllerSpiffe, cluster: &str, name: &str) -> bool {
    parsed.cluster == cluster && parsed.name == name
}

#[cfg(test)]
#[path = "../../tests/unit/fed_sync/spiffe_test.rs"]
mod tests;
