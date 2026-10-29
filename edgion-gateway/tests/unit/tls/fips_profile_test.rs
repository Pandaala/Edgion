use super::*;

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
    assert_eq!(FIPS_GROUP_IDS, &[23, 24, 25]);
    assert_eq!(FIPS_CURVES, "P-256:P-384:P-521");
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
    crate::tls::downstream_policy::CipherPolicy::Inherit
        .apply(ssl.as_mut(), true)
        .expect("approved fips ciphers must apply on a boringssl build");
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
            crate::tls::downstream_policy::CipherPolicy::Inherit
                .apply(ssl.as_mut(), true)
                .unwrap();
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
