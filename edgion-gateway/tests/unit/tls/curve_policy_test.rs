use super::*;
use edgion_resources::resources::downstream_tls::parse_ecdh_curves;

fn policy(names: &[&str]) -> CurvePolicy {
    CurvePolicy::compile(parse_ecdh_curves(names.iter().copied()).map(Some))
}

#[cfg(feature = "boringssl")]
#[test]
fn numeric_policy_and_fips_subset_apply_without_broadening() {
    use boring::ssl::{Ssl, SslContext, SslMethod};
    let context = SslContext::builder(SslMethod::tls()).unwrap().build();
    let mut ssl = Ssl::new(&context).unwrap();
    let subset = policy(&["P-384", "P-256"]);
    assert!(matches!(&subset, CurvePolicy::Explicit { groups, fips_compatible: true } if groups == &[24,23]));
    subset.apply(&mut ssl, true).unwrap();
    assert!(policy(&["X25519", "P-256"]).apply(&mut ssl, true).is_err());
    policy(&["X25519", "P-256"]).apply(&mut ssl, false).unwrap();
    CurvePolicy::Inherit.apply(&mut ssl, true).unwrap();
    CurvePolicy::Inherit.apply(&mut ssl, false).unwrap();
    assert!(CurvePolicy::Reject("invalid curves").apply(&mut ssl, false).is_err());
    assert!(set_groups(&mut ssl, &[u16::MAX]).is_err());
}

#[cfg(feature = "boringssl")]
#[test]
fn early_numeric_curve_policy_negotiates_tls12_and_tls13_in_memory() {
    use boring::asn1::Asn1Time;
    use boring::hash::MessageDigest;
    use boring::pkey::PKey;
    use boring::rsa::Rsa;
    use boring::ssl::{ErrorCode, SelectCertError, Ssl, SslContext, SslMethod, SslStream, SslVerifyMode, SslVersion};
    use boring::x509::{X509NameBuilder, X509};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::rc::Rc;

    // Paired in-memory byte queues exercise real BoringSSL handshakes without
    // a network listener, external server or global FIPS state.
    struct MemoryStream {
        incoming: Rc<RefCell<VecDeque<u8>>>,
        outgoing: Rc<RefCell<VecDeque<u8>>>,
    }
    impl Read for MemoryStream {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            if target.is_empty() {
                return Ok(0);
            }
            let mut incoming = self.incoming.borrow_mut();
            if incoming.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = target.len().min(incoming.len());
            for slot in &mut target[..count] {
                *slot = incoming.pop_front().unwrap();
            }
            Ok(count)
        }
    }
    impl Write for MemoryStream {
        fn write(&mut self, source: &[u8]) -> io::Result<usize> {
            self.outgoing.borrow_mut().extend(source);
            Ok(source.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "curve.test").unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(&name).unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
    cert.sign(&key, MessageDigest::sha256()).unwrap();
    let cert = cert.build();
    for version in [SslVersion::TLS1_2, SslVersion::TLS1_3] {
        for offered in ["P-384", "P-256"] {
            let mut server = SslContext::builder(SslMethod::tls()).unwrap();
            server.set_min_proto_version(Some(version)).unwrap();
            server.set_max_proto_version(Some(version)).unwrap();
            server.set_certificate(&cert).unwrap();
            server.set_private_key(&key).unwrap();
            let selected = policy(&["P-384"]);
            server.set_select_certificate_callback(move |mut hello| {
                selected
                    .apply(hello.ssl_mut(), true)
                    .map_err(|_| SelectCertError::ERROR)
            });
            let server = server.build();
            let mut client = SslContext::builder(SslMethod::tls()).unwrap();
            client.set_min_proto_version(Some(version)).unwrap();
            client.set_max_proto_version(Some(version)).unwrap();
            client.set_verify(SslVerifyMode::NONE);
            // Static RSA suites do not use ECDH and would make a disjoint
            // group offer succeed for an unrelated key-exchange mechanism.
            client.set_cipher_list("ECDHE-RSA-AES128-GCM-SHA256").unwrap();
            let client = client.build();
            let mut client_ssl = Ssl::new(&client).unwrap();
            policy(&[offered]).apply(&mut client_ssl, false).unwrap();
            let a = Rc::new(RefCell::new(VecDeque::new()));
            let b = Rc::new(RefCell::new(VecDeque::new()));
            let mut server = SslStream::new(
                Ssl::new(&server).unwrap(),
                MemoryStream {
                    incoming: a.clone(),
                    outgoing: b.clone(),
                },
            )
            .unwrap();
            let mut client = SslStream::new(
                client_ssl,
                MemoryStream {
                    incoming: b,
                    outgoing: a,
                },
            )
            .unwrap();
            let (mut client_done, mut server_done) = (false, false);
            for _ in 0..32 {
                if !client_done {
                    match client.connect() {
                        Ok(()) => client_done = true,
                        Err(e) if e.code() == ErrorCode::WANT_READ => {}
                        Err(_) => break,
                    }
                }
                if !server_done {
                    match server.accept() {
                        Ok(()) => server_done = true,
                        Err(e) if e.code() == ErrorCode::WANT_READ => {}
                        Err(_) => break,
                    }
                }
                if client_done && server_done {
                    break;
                }
            }
            assert_eq!(
                client_done && server_done,
                offered == "P-384",
                "version {version:?}, offered {offered}; FIPS subset must not widen to P-256"
            );
        }
    }
}
