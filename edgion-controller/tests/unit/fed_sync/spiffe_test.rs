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
fn self_check_path_matches_cluster_name() {
    let p = ControllerSpiffe {
        host: "any".into(),
        cluster: "prod".into(),
        name: "ctrl-1".into(),
    };
    assert!(path_matches(&p, "prod", "ctrl-1"));
    assert!(!path_matches(&p, "prod", "ctrl-2"));
}
