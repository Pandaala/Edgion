use super::*;

fn ca_pem() -> String {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().unwrap();
    params.self_signed(&key).unwrap().pem()
}

fn write_temp(content: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, content).unwrap();
    file
}

#[test]
fn loaded_bundle_shares_one_root_store_and_round_trips_pem() {
    let bundle = format!("{}\n{}", ca_pem(), ca_pem());
    let file = write_temp(bundle.as_bytes());
    let trust = SystemTrust::load(file.path()).unwrap();

    assert_eq!(trust.source(), &SystemTrustSource::Loaded(file.path().to_path_buf()));
    assert_eq!(trust.bundle_file(), Some(file.path()));
    assert_eq!(trust.certificates().len(), 2);
    assert_eq!(trust.skipped_certificates(), 0);
    assert_eq!(trust.root_store().len(), 2);
    // Clients share one store instead of copying anchors.
    assert!(Arc::ptr_eq(&trust.root_store(), &trust.root_store()));

    // The re-encoded PEM handed to tonic/redis-rs carries the same certificates.
    let reparsed = CertificateDer::pem_slice_iter(trust.pem())
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(reparsed, trust.certificates());
}

#[test]
fn unusable_certificates_are_skipped_counted_and_excluded_from_pem() {
    // Well-formed PEM whose DER rustls rejects, plus labels rustls never loads.
    let bundle = format!(
        "{}-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
         -----BEGIN TRUSTED CERTIFICATE-----\nAAAA\n-----END TRUSTED CERTIFICATE-----\n\
         -----BEGIN X509 CRL-----\nAAAA\n-----END X509 CRL-----\n",
        ca_pem()
    );
    let file = write_temp(bundle.as_bytes());
    let trust = SystemTrust::load(file.path()).unwrap();

    assert_eq!(trust.certificates().len(), 1);
    assert_eq!(trust.skipped_certificates(), 2);
    let reparsed = CertificateDer::pem_slice_iter(trust.pem())
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(reparsed.len(), 1);
}

#[test]
fn bundle_without_certificates_is_rejected() {
    let file = write_temp(b"not a certificate bundle\n");
    let error = SystemTrust::load(file.path()).err().unwrap();
    assert!(format!("{error:#}").contains("no usable certificates"));
}

#[test]
fn bundle_with_malformed_pem_is_rejected() {
    let file = write_temp(b"-----BEGIN CERTIFICATE-----\n@@@@\n-----END CERTIFICATE-----\n");
    assert!(SystemTrust::load(file.path()).is_err());
}

#[test]
fn missing_bundle_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SystemTrust::load(&dir.path().join("absent.pem")).is_err());
}

#[test]
fn non_regular_bundle_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let error = SystemTrust::load(dir.path()).err().unwrap();
    assert!(format!("{error:#}").contains("not a regular file"));
}

#[test]
fn oversized_bundle_is_rejected() {
    let file = write_temp(&vec![b'\n'; MAX_BUNDLE_BYTES as usize + 1]);
    let error = SystemTrust::load(file.path()).err().unwrap();
    assert!(format!("{error:#}").contains("exceeds"));
}

#[test]
fn unusable_bundle_yields_an_empty_fail_closed_set() {
    let dir = tempfile::tempdir().unwrap();
    let trust = SystemTrust::load_or_unavailable(&dir.path().join("absent.pem"));
    assert!(matches!(trust.source(), SystemTrustSource::Unavailable(_)));
    assert!(trust.is_empty());
    assert!(trust.pem().is_empty());
    assert_eq!(trust.bundle_file(), None);
    assert!(trust.root_store().is_empty());
}

#[test]
fn bundle_selection_takes_the_first_existing_path_in_fixed_order() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.pem");
    let first = write_temp(b"first");
    let second = write_temp(b"second");
    let candidates = [
        missing.to_str().unwrap(),
        first.path().to_str().unwrap(),
        second.path().to_str().unwrap(),
    ];
    assert_eq!(select_bundle(&candidates).unwrap(), first.path());
    let error = select_bundle(&candidates[..1]).err().unwrap();
    assert!(format!("{error:#}").contains("no OS CA bundle found"));
}

#[test]
fn a_broken_higher_priority_bundle_is_selected_not_skipped() {
    // A directory or dangling symlink at a candidate path is the bundle; loading
    // it then fails instead of silently falling through to the next path.
    let dir = tempfile::tempdir().unwrap();
    let valid = write_temp(ca_pem().as_bytes());
    let candidates = [dir.path().to_str().unwrap(), valid.path().to_str().unwrap()];
    let selected = select_bundle(&candidates).unwrap();
    assert_eq!(selected, dir.path());
    assert!(SystemTrust::load(&selected).is_err());

    #[cfg(unix)]
    {
        let dangling = dir.path().join("dangling.pem");
        std::os::unix::fs::symlink(dir.path().join("absent.pem"), &dangling).unwrap();
        let candidates = [dangling.to_str().unwrap(), valid.path().to_str().unwrap()];
        assert_eq!(select_bundle(&candidates).unwrap(), dangling);
    }
}

#[test]
fn process_trust_reads_only_the_fixed_os_bundle_paths() {
    // Selection is fixed: no configuration or SSL_CERT_FILE can redirect it.
    match system_trust().source() {
        SystemTrustSource::Loaded(path) => assert!(SYSTEM_CA_FILES.iter().any(|fixed| path == Path::new(fixed))),
        SystemTrustSource::Unavailable(reason) => assert!(!reason.is_empty()),
    }
}
