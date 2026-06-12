//! Pure SPIFFE URI-SAN parsing and matching for the federation plane.
//! Shared by the Center server (identity binding) and the Controller
//! startup self-check. No I/O — operates on DER bytes the caller already holds.

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

/// Center-side exact match: host == trust_domain (case-insensitive),
/// cluster == request cluster (case-sensitive), and
/// `controller_id == "{cluster}/{name}"` (case-sensitive).
pub fn check_match(
    parsed: &ControllerSpiffe,
    trust_domain: &str,
    req_cluster: &str,
    req_controller_id: &str,
) -> Result<(), PeerIdentityError> {
    if !parsed.host.eq_ignore_ascii_case(trust_domain) {
        return Err(PeerIdentityError::Mismatch);
    }
    if parsed.cluster != req_cluster {
        return Err(PeerIdentityError::Mismatch);
    }
    if format!("{}/{}", parsed.cluster, parsed.name) != req_controller_id {
        return Err(PeerIdentityError::Mismatch);
    }
    Ok(())
}

/// Full Center verify: extract -> parse -> match. Single entry point for `sync()`.
pub fn verify(
    leaf_der: &[u8],
    trust_domain: &str,
    req_cluster: &str,
    req_controller_id: &str,
) -> Result<(), PeerIdentityError> {
    let uri = extract_single_spiffe_uri(leaf_der)?;
    let parsed = parse_controller_spiffe(&uri).ok_or(PeerIdentityError::Mismatch)?;
    check_match(&parsed, trust_domain, req_cluster, req_controller_id)
}

/// Controller-side path-only check (trust_domain verified by Center).
pub fn path_matches(parsed: &ControllerSpiffe, cluster: &str, name: &str) -> bool {
    parsed.cluster == cluster && parsed.name == name
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Real-DER helpers (rcgen 0.14.7) ──────────────────────────────────────

    /// Build a self-signed cert DER carrying the given URI and DNS SANs.
    ///
    /// Uses rcgen 0.14.7: `SanType::URI(Ia5String)` for URI SANs,
    /// `SanType::DnsName(Ia5String)` for DNS SANs. `cert.der()` returns
    /// `&CertificateDer<'static>` which derefs to `&[u8]`.
    #[cfg(test)]
    fn cert_der_with_sans(uri_sans: &[&str], dns_sans: &[&str]) -> Vec<u8> {
        use rcgen::{CertificateParams, KeyPair, SanType};

        let key = KeyPair::generate().expect("keypair");
        let mut params = CertificateParams::new(vec![]).expect("params");
        // `Ia5String` is not re-exported; use `TryFrom<&str>` via `.try_into()`.
        for u in uri_sans {
            params
                .subject_alt_names
                .push(SanType::URI((*u).try_into().expect("ia5 uri")));
        }
        for d in dns_sans {
            params
                .subject_alt_names
                .push(SanType::DnsName((*d).try_into().expect("ia5 dns")));
        }
        let cert = params.self_signed(&key).expect("self-signed");
        cert.der().to_vec()
    }

    // ── Real-DER tests ────────────────────────────────────────────────────────

    #[test]
    fn real_cert_single_spiffe_san_extracts() {
        let der = cert_der_with_sans(&["spiffe://edgion.io/controllers/prod/ctrl-1"], &[]);
        let uri = extract_single_spiffe_uri(&der).expect("one spiffe san");
        assert_eq!(uri, "spiffe://edgion.io/controllers/prod/ctrl-1");
    }

    #[test]
    fn real_cert_two_spiffe_sans_rejected() {
        let der = cert_der_with_sans(
            &[
                "spiffe://edgion.io/controllers/prod/ctrl-1",
                "spiffe://edgion.io/controllers/attacker/x",
            ],
            &[],
        );
        assert_eq!(extract_single_spiffe_uri(&der), Err(PeerIdentityError::MultiSan));
    }

    #[test]
    fn real_cert_no_spiffe_san_rejected() {
        let der = cert_der_with_sans(&[], &["edge.example.com"]);
        assert_eq!(extract_single_spiffe_uri(&der), Err(PeerIdentityError::NoSpiffeSan));
    }

    #[test]
    fn real_cert_dns_plus_one_spiffe_ok() {
        let der = cert_der_with_sans(&["spiffe://edgion.io/controllers/prod/ctrl-1"], &["edge.example.com"]);
        let uri = extract_single_spiffe_uri(&der).expect("dns ignored, one spiffe");
        assert_eq!(uri, "spiffe://edgion.io/controllers/prod/ctrl-1");
    }

    #[test]
    fn real_cert_verify_full_match_ok() {
        let der = cert_der_with_sans(&["spiffe://edgion.io/controllers/prod/ctrl-1"], &[]);
        assert_eq!(verify(&der, "edgion.io", "prod", "prod/ctrl-1"), Ok(()));
    }

    #[test]
    fn real_cert_verify_wrong_controller_id_rejected() {
        let der = cert_der_with_sans(&["spiffe://edgion.io/controllers/attacker/x"], &[]);
        assert_eq!(
            verify(&der, "edgion.io", "prod", "prod/ctrl-1"),
            Err(PeerIdentityError::Mismatch)
        );
    }

    #[test]
    fn real_cert_garbage_der_parse_error() {
        let garbage = vec![0x30, 0x03, 0x01, 0x02, 0x03];
        assert_eq!(extract_single_spiffe_uri(&garbage), Err(PeerIdentityError::ParseError));
    }

    // ── Original string-level tests ───────────────────────────────────────────

    #[test]
    fn parse_accepts_well_formed() {
        let p = parse_controller_spiffe("spiffe://edgion.io/controllers/prod/ctrl-1").unwrap();
        assert_eq!(p.host, "edgion.io");
        assert_eq!(p.cluster, "prod");
        assert_eq!(p.name, "ctrl-1");
    }

    #[test]
    fn parse_rejects_wrong_segment_count() {
        assert!(parse_controller_spiffe("spiffe://edgion.io/controllers/prod/ctrl/extra").is_none());
        assert!(parse_controller_spiffe("spiffe://edgion.io/controllers/prod").is_none());
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        assert!(parse_controller_spiffe("spiffe://edgion.io/agents/prod/ctrl-1").is_none());
    }

    #[test]
    fn parse_rejects_empty_segment_and_trailing_slash() {
        assert!(parse_controller_spiffe("spiffe://edgion.io/controllers//ctrl-1").is_none());
        assert!(parse_controller_spiffe("spiffe://edgion.io/controllers/prod/ctrl-1/").is_none());
    }

    #[test]
    fn parse_accepts_case_insensitive_scheme() {
        assert!(parse_controller_spiffe("SPIFFE://edgion.io/controllers/prod/ctrl-1").is_some());
    }

    #[test]
    fn parse_does_not_decode_percent() {
        let p = parse_controller_spiffe("spiffe://edgion.io/controllers/prod/ctrl%2F1").unwrap();
        assert_eq!(p.name, "ctrl%2F1");
    }

    #[test]
    fn match_fields_ok() {
        let p = ControllerSpiffe {
            host: "edgion.io".into(),
            cluster: "prod".into(),
            name: "ctrl-1".into(),
        };
        assert_eq!(check_match(&p, "edgion.io", "prod", "prod/ctrl-1"), Ok(()));
    }

    #[test]
    fn match_rejects_trust_domain_trailing_dot() {
        let p = ControllerSpiffe {
            host: "edgion.io.".into(),
            cluster: "prod".into(),
            name: "ctrl-1".into(),
        };
        assert_eq!(
            check_match(&p, "edgion.io", "prod", "prod/ctrl-1"),
            Err(PeerIdentityError::Mismatch)
        );
    }

    #[test]
    fn match_trust_domain_case_insensitive() {
        let p = ControllerSpiffe {
            host: "Edgion.IO".into(),
            cluster: "prod".into(),
            name: "ctrl-1".into(),
        };
        assert_eq!(check_match(&p, "edgion.io", "prod", "prod/ctrl-1"), Ok(()));
    }

    #[test]
    fn match_rejects_cluster_mismatch() {
        let p = ControllerSpiffe {
            host: "edgion.io".into(),
            cluster: "staging".into(),
            name: "ctrl-1".into(),
        };
        assert_eq!(
            check_match(&p, "edgion.io", "staging", "prod/ctrl-1"),
            Err(PeerIdentityError::Mismatch)
        );
    }

    #[test]
    fn match_rejects_empty_cluster_request() {
        let p = ControllerSpiffe {
            host: "edgion.io".into(),
            cluster: "prod".into(),
            name: "ctrl-1".into(),
        };
        assert_eq!(
            check_match(&p, "edgion.io", "", "/ctrl-1"),
            Err(PeerIdentityError::Mismatch)
        );
    }

    #[test]
    fn self_check_path_matches_cluster_name() {
        let p = ControllerSpiffe {
            host: "any".into(),
            cluster: "prod".into(),
            name: "ctrl-1".into(),
        };
        assert!(path_matches(&p, "prod", "ctrl-1"));
        assert!(!path_matches(&p, "prod", "ctrl-2"));
    }
}
