use super::*;
use clap::Parser;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ca_pem() -> String {
    rcgen::generate_simple_self_signed(vec!["test-ca".into()])
        .unwrap()
        .cert
        .pem()
}

fn file_with(contents: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(contents).unwrap();
    file
}

fn load_error(contents: &[u8]) -> String {
    let file = file_with(contents);
    format!("{:#}", ServerTrust::from_ca_cert(Some(file.path())).unwrap_err())
}

/// A private CA and a `localhost` server certificate it issued.
struct PrivatePki {
    ca: String,
    cert: String,
    key: String,
}

impl PrivatePki {
    fn new() -> Self {
        use rcgen::*;
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
        let ca_key = KeyPair::generate().unwrap();
        let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
        let issuer = Issuer::new(ca, ca_key);
        let mut leaf = CertificateParams::new(vec!["localhost".into()]).unwrap();
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_key = KeyPair::generate().unwrap();
        Self {
            ca: ca_pem,
            cert: leaf.signed_by(&leaf_key, &issuer).unwrap().pem(),
            key: leaf_key.serialize_pem(),
        }
    }
}

/// HTTPS server answering every request with an empty resource list.
async fn serve_https(pki: &PrivatePki) -> (String, tokio::task::JoinHandle<()>) {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            CertificateDer::pem_slice_iter(pki.cert.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap(),
            PrivateKeyDer::from_pem_slice(pki.key.as_bytes()).unwrap(),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://localhost:{}", listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(socket).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                let body = br#"{"success":true,"data":[]}"#;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (url, task)
}

#[test]
fn absent_or_empty_ca_cert_keeps_system_trust() {
    assert!(matches!(ServerTrust::from_ca_cert(None).unwrap(), ServerTrust::System));
    assert!(matches!(
        ServerTrust::from_ca_cert(Some(Path::new(""))).unwrap(),
        ServerTrust::System
    ));
}

/// An empty `--ca-cert` (or `EDGION_CA_CERT=`) means unset, like `EDGION_TOKEN=`.
#[tokio::test]
async fn empty_ca_cert_value_means_unset() {
    let cli = crate::cli::Cli::try_parse_from(["edgion-cli", "--ca-cert", "", "gen-token"])
        .expect("an empty --ca-cert must parse");
    assert_eq!(cli.ca_cert.as_deref(), Some(Path::new("")));
    assert!(matches!(
        ServerTrust::from_ca_cert(cli.ca_cert.as_deref()).unwrap(),
        ServerTrust::System
    ));
    cli.run().await.expect("an empty --ca-cert must not break gen-token");
}

#[test]
fn ca_cert_file_is_loaded_and_builds_a_client() {
    let file = file_with(ca_pem().as_bytes());
    let trust = ServerTrust::from_ca_cert(Some(file.path())).unwrap();
    match &trust {
        ServerTrust::CaFile { path, .. } => assert_eq!(path, file.path()),
        ServerTrust::System => panic!("--ca-cert must select the CA file"),
    }
    build_cli_client(&trust).expect("client with a CA file");
}

#[test]
fn debug_output_names_only_the_path() {
    let file = file_with(ca_pem().as_bytes());
    let trust = ServerTrust::from_ca_cert(Some(file.path())).unwrap();
    let rendered = format!("{trust:?}");
    assert!(rendered.contains("CaFile"), "{rendered}");
    assert!(!rendered.contains("BEGIN"), "{rendered}");
}

/// Distribution and curl bundles carry comments or `openssl x509 -text` output
/// between certificate blocks; such a file is accepted.
#[test]
fn ca_cert_file_with_text_between_blocks_is_accepted() {
    let pem = format!(
        "# Bundle comment\n{}\nCertificate:\n    Data: text dump\n{}",
        ca_pem(),
        ca_pem()
    );
    let trust = ServerTrust::from_ca_cert(Some(file_with(pem.as_bytes()).path())).unwrap();
    build_cli_client(&trust).expect("client with a commented CA file");
}

/// Duplicates do not count toward the canonical certificate limit.
#[test]
fn repeated_certificates_are_deduplicated() {
    let pem = ca_pem().repeat(1100);
    assert!(matches!(
        ServerTrust::from_ca_cert(Some(file_with(pem.as_bytes()).path())).unwrap(),
        ServerTrust::CaFile { .. }
    ));
}

#[test]
fn trusted_certificate_blocks_are_rejected_with_a_conversion_hint() {
    let trusted = ca_pem().replace("CERTIFICATE-----", "TRUSTED CERTIFICATE-----");
    let error = load_error(format!("{}{}", ca_pem(), trusted).as_bytes());
    assert!(error.contains("TRUSTED CERTIFICATE"), "{error}");
    assert!(error.contains("openssl storeutl -certs"), "{error}");

    // Only a label at the start of a line begins a block; a comment that
    // mentions one is ordinary text.
    let commented = format!(
        "# convert -----BEGIN TRUSTED CERTIFICATE----- blocks first\n{}",
        ca_pem()
    );
    assert!(matches!(
        ServerTrust::from_ca_cert(Some(file_with(commented.as_bytes()).path())).unwrap(),
        ServerTrust::CaFile { .. }
    ));
}

#[test]
fn missing_ca_cert_file_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing-ca.pem");
    let error = format!("{:#}", ServerTrust::from_ca_cert(Some(&path)).unwrap_err());
    assert!(error.contains("failed to read CA certificate file"), "{error}");
    assert!(error.contains("missing-ca.pem"), "{error}");
}

#[test]
fn invalid_ca_cert_files_are_rejected_with_a_readable_reason() {
    for (contents, reason) in [
        (&b""[..], "no CERTIFICATE block"),
        (b"not a certificate", "no CERTIFICATE block"),
        (b"-----BEGIN CERTIFICATE-----\nAAAA\n", "no END line"),
        (
            b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n",
            "invalid base64",
        ),
    ] {
        let error = load_error(contents);
        assert!(error.contains("invalid CA certificate file"), "{error}");
        assert!(error.contains(reason), "{error}");
        assert!(!error.contains("[45, 45"), "raw PEM bytes in: {error}");
    }
}

#[test]
fn oversized_ca_cert_file_is_rejected() {
    let limit = edgion_common::system_trust::MAX_CA_FILE_BYTES as usize;
    let error = load_error(&vec![b'a'; limit + 1]);
    assert!(error.contains("exceeds"), "{error}");
}

#[test]
fn ca_cert_flag_parses_before_the_subcommand() {
    let cli = crate::cli::Cli::try_parse_from(["edgion-cli", "--ca-cert", "/tmp/ca.pem", "get", "httproute"])
        .expect("--ca-cert should parse");
    assert_eq!(cli.ca_cert.as_deref(), Some(Path::new("/tmp/ca.pem")));
}

/// Local-only commands must not read `--ca-cert`, so a bad value cannot break them.
#[tokio::test]
async fn local_only_commands_never_read_the_ca_cert() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-ca.pem");
    let missing = missing.to_str().unwrap();
    let wasm = dir.path().join("demo.wasm");
    std::fs::write(
        &wasm,
        wat::parse_str(r#"(module (func (export "proxy_abi_version_0_2_0")))"#).unwrap(),
    )
    .unwrap();
    for args in [
        vec!["edgion-cli", "--ca-cert", missing, "gen-token"],
        vec![
            "edgion-cli",
            "--ca-cert",
            missing,
            "wasm",
            "inspect",
            "--file",
            wasm.to_str().unwrap(),
        ],
    ] {
        let cli = crate::cli::Cli::try_parse_from(args.clone()).unwrap();
        cli.run().await.unwrap_or_else(|error| panic!("{args:?}: {error:#}"));
    }
}

/// Commands that contact the server read `--ca-cert` before any request.
#[tokio::test]
async fn server_commands_read_the_ca_cert_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-ca.pem");
    let cli = crate::cli::Cli::try_parse_from([
        "edgion-cli",
        "--ca-cert",
        missing.to_str().unwrap(),
        "--server",
        "http://127.0.0.1:1",
        "--token",
        "edg_test",
        "get",
        "httproute",
    ])
    .unwrap();
    let error = format!("{:#}", cli.run().await.unwrap_err());
    assert!(error.contains("failed to read CA certificate file"), "{error}");
}

/// `login` never lets an unusable CA file block saving the token.
#[test]
fn login_probe_skips_an_unusable_ca_cert() {
    let dir = tempfile::tempdir().unwrap();
    assert!(login_probe_trust(Some(&dir.path().join("missing-ca.pem"))).is_none());
    assert!(matches!(login_probe_trust(None), Some(ServerTrust::System)));
}

/// End to end through `Cli::run`: `--ca-cert` reaches the request path, and
/// without it the private-CA server is rejected with a `--ca-cert` hint.
#[tokio::test]
async fn ca_cert_is_used_for_https_requests() {
    let pki = PrivatePki::new();
    let (url, server) = serve_https(&pki).await;
    let ca_file = file_with(pki.ca.as_bytes());
    let run = |ca_cert: Option<&str>| {
        let mut args = vec!["edgion-cli", "--server", url.as_str(), "--token", "edg_test"];
        if let Some(path) = ca_cert {
            args.extend(["--ca-cert", path]);
        }
        args.extend(["get", "httproute"]);
        crate::cli::Cli::try_parse_from(args).unwrap()
    };
    run(Some(ca_file.path().to_str().unwrap()))
        .run()
        .await
        .expect("the private CA must verify the server");
    assert!(run(None).run().await.is_err(), "System trust must reject a private CA");

    let error = build_cli_client(&ServerTrust::System)
        .unwrap()
        .get(format!("{url}/api/v1/auth/status"))
        .send()
        .await
        .unwrap_err();
    let message = format_request_error(&url, error, TargetType::Center);
    assert!(message.contains("certificate"), "{message}");
    assert!(message.contains("--ca-cert"), "{message}");
    server.abort();
}
