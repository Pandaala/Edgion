use super::*;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Cursor, Write};

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(crate) fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (name, data) in entries {
        append_entry(&mut builder, name, data, tar::EntryType::Regular);
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn append_entry<W: Write>(builder: &mut tar::Builder<W>, name: &str, data: &[u8], entry_type: tar::EntryType) {
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(data.len()).unwrap());
    header.set_mode(0o644);
    header.set_entry_type(entry_type);
    header.set_cksum();
    builder.append_data(&mut header, name, data).unwrap();
}

fn build_custom_tar_gz(build: impl FnOnce(&mut tar::Builder<GzEncoder<Vec<u8>>>)) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    build(&mut builder);
    builder.into_inner().unwrap().finish().unwrap()
}

fn pax_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!(" {key}={value}\n");
    let mut digits = 1usize;
    loop {
        let length = digits + body.len();
        let next_digits = length.to_string().len();
        if next_digits == digits {
            return format!("{length}{body}").into_bytes();
        }
        digits = next_digits;
    }
}

fn append_pax<W: Write>(builder: &mut tar::Builder<W>, payload: &[u8], entry_type: tar::EntryType) {
    append_entry(builder, "PaxHeaders/entry", payload, entry_type);
}

fn descriptors(count: usize) -> Vec<serde_json::Value> {
    (0..count).map(|_| serde_json::json!({"digest": DIGEST_A})).collect()
}

fn manifest_json(layers: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": layers
    }))
    .unwrap()
}

#[test]
fn parses_image_and_index() {
    let img = manifest_json(serde_json::json!([
        {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": DIGEST_A, "size": 8}
    ]));
    assert!(matches!(parse_manifest(&img).unwrap(), ParsedManifest::Image(_)));

    let idx = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MEDIA_TYPE_OCI_INDEX,
        "manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": DIGEST_B}]
    }))
    .unwrap();
    match parse_manifest(&idx).unwrap() {
        ParsedManifest::Index(entries) => assert_eq!(entries[0].digest, DIGEST_B),
        ParsedManifest::Image(_) => panic!("expected index"),
    }
    assert!(parse_manifest(b"{not json").is_err());
}

#[test]
fn validates_schema_shape_media_type_and_unknown_fields() {
    let descriptor = serde_json::json!({"digest": DIGEST_A, "future": {"nested": true}});
    let inferred = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "layers": [descriptor.clone()],
        "future": [1, 2, 3]
    }))
    .unwrap();
    assert!(matches!(parse_manifest(&inferred).unwrap(), ParsedManifest::Image(_)));

    for invalid in [
        serde_json::json!({"layers": [descriptor.clone()]}),
        serde_json::json!({"schemaVersion": 1, "layers": [descriptor.clone()]}),
        serde_json::json!({"schemaVersion": 2}),
        serde_json::json!({"schemaVersion": 2, "layers": [descriptor.clone()], "manifests": [descriptor.clone()]}),
        serde_json::json!({"schemaVersion": 2, "mediaType": MEDIA_TYPE_OCI_INDEX, "layers": [descriptor.clone()]}),
        serde_json::json!({"schemaVersion": 2, "mediaType": "application/example", "layers": [descriptor.clone()]}),
    ] {
        assert!(parse_manifest(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}

#[test]
fn validates_and_normalizes_descriptor_digests() {
    let uppercase = format!("sha256:{}", "AB".repeat(32));
    let manifest = manifest_json(serde_json::json!([{"digest": uppercase}]));
    let ParsedManifest::Image(image) = parse_manifest(&manifest).unwrap() else {
        panic!("expected image");
    };
    assert_eq!(image.layers[0].digest, format!("sha256:{}", "ab".repeat(32)));

    for digest in [
        "sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sha256:aa",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ",
    ] {
        let manifest = manifest_json(serde_json::json!([{"digest": digest}]));
        assert!(parse_manifest(&manifest).err().unwrap().contains("digest"));
    }
}

#[test]
fn descriptor_budget_is_shared_and_precedes_shape_errors_in_both_orders() {
    let exact_half = serde_json::to_string(&descriptors(MAX_OCI_DESCRIPTORS / 2)).unwrap();
    let over_half = serde_json::to_string(&descriptors(MAX_OCI_DESCRIPTORS / 2 + 1)).unwrap();
    for (first_name, second_name) in [("layers", "manifests"), ("manifests", "layers")] {
        let exact = format!("{{\"schemaVersion\":2,\"{first_name}\":{exact_half},\"{second_name}\":{exact_half}}}");
        let error = parse_manifest(exact.as_bytes()).err().unwrap();
        assert!(error.contains("both layers and manifests"), "got: {error}");

        let over = format!("{{\"schemaVersion\":2,\"{first_name}\":{exact_half},\"{second_name}\":{over_half}}}");
        let error = parse_manifest(over.as_bytes()).err().unwrap();
        assert!(error.contains("descriptor limit"), "got: {error}");
        assert!(!error.contains("both layers"), "got: {error}");
    }

    let exact = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "layers": descriptors(MAX_OCI_DESCRIPTORS)
    }))
    .unwrap();
    assert!(serde_json::from_slice::<OciManifest>(&exact).is_ok());

    let over = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "layers": descriptors(MAX_OCI_DESCRIPTORS + 1)
    }))
    .unwrap();
    let error = serde_json::from_slice::<OciManifest>(&over).unwrap_err();
    assert!(error.to_string().contains("descriptor limit"));
}

#[test]
fn selects_wasm_layer_preferring_media_type() {
    let m: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
        {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": DIGEST_A},
        {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": DIGEST_B}
    ])))
    .unwrap();
    match select_wasm_layer(&m).unwrap() {
        WasmLayer::Raw(d) => assert_eq!(d.digest, DIGEST_B),
        WasmLayer::TarGz(_) => panic!("expected raw wasm layer"),
    }

    let multiple: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
        {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": DIGEST_A},
        {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": DIGEST_B}
    ])))
    .unwrap();
    assert!(select_wasm_layer(&multiple).unwrap_err().contains("multiple raw wasm"));
}

#[test]
fn single_targz_layer_is_compat_and_multi_layer_without_wasm_errors() {
    let single: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
        {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": DIGEST_A}
    ])))
    .unwrap();
    assert!(matches!(select_wasm_layer(&single).unwrap(), WasmLayer::TarGz(_)));

    let multi: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
        {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": DIGEST_A},
        {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": DIGEST_B}
    ])))
    .unwrap();
    let err = select_wasm_layer(&multi).unwrap_err();
    assert!(err.contains("no wasm layer"), "got: {err}");
}

#[test]
fn extracts_plugin_wasm_from_tar_gz() {
    let wasm = b"\0asm\x01\x00\x00\x00";
    let layer = build_tar_gz(&[("etc/other.txt", b"x"), ("plugin.wasm", wasm)]);
    assert_eq!(extract_wasm_from_tar_gz(&layer).unwrap(), wasm);
    let layer = build_tar_gz(&[("./plugin.wasm", wasm)]);
    assert_eq!(extract_wasm_from_tar_gz(&layer).unwrap(), wasm);
    let layer = build_tar_gz(&[("nothing.here", b"x")]);
    assert!(extract_wasm_from_tar_gz(&layer).unwrap_err().contains("plugin.wasm"));
    assert!(extract_wasm_from_tar_gz(b"not-gzip").is_err());
}

#[test]
fn rejects_duplicate_non_file_and_unsafe_plugin_candidates() {
    let duplicate = build_tar_gz(&[("plugin.wasm", b"a"), ("dir/plugin.wasm", b"b")]);
    assert!(extract_wasm_from_tar_gz(&duplicate).unwrap_err().contains("multiple"));

    for entry_type in [tar::EntryType::Symlink, tar::EntryType::Link, tar::EntryType::Directory] {
        let non_file = build_custom_tar_gz(|builder| {
            append_entry(builder, "plugin.wasm", b"", entry_type);
        });
        assert!(extract_wasm_from_tar_gz(&non_file)
            .unwrap_err()
            .contains("regular file"));
    }

    for path in ["../plugin.wasm", "/plugin.wasm"] {
        let pax = pax_record("path", path);
        let unsafe_path = build_custom_tar_gz(|builder| {
            append_pax(builder, &pax, tar::EntryType::XHeader);
            append_entry(builder, "placeholder", b"wasm", tar::EntryType::Regular);
        });
        assert!(extract_wasm_from_tar_gz(&unsafe_path)
            .unwrap_err()
            .contains("unsafe archive path"));
    }
}

#[test]
fn enforces_declared_output_extension_and_total_boundaries() {
    assert!(validate_declared_entry_size(MAX_DECLARED_ENTRY_BYTES).is_ok());
    assert!(validate_declared_entry_size(MAX_DECLARED_ENTRY_BYTES + 1).is_err());

    let mut exact_extension = Cursor::new(vec![0; MAX_ARCHIVE_EXTENSION_BYTES as usize]);
    assert_eq!(
        read_extension_payload(&mut exact_extension, MAX_ARCHIVE_EXTENSION_BYTES)
            .unwrap()
            .len(),
        MAX_ARCHIVE_EXTENSION_BYTES as usize
    );
    assert!(read_extension_payload(&mut Cursor::new(Vec::<u8>::new()), MAX_ARCHIVE_EXTENSION_BYTES + 1).is_err());

    let mut exact_output = io::repeat(0).take(MAX_EXTRACTED_WASM_BYTES);
    assert_eq!(
        read_wasm_entry(&mut exact_output, MAX_EXTRACTED_WASM_BYTES)
            .unwrap()
            .len(),
        MAX_EXTRACTED_WASM_BYTES as usize
    );
    assert!(read_wasm_entry(&mut io::empty(), MAX_EXTRACTED_WASM_BYTES + 1).is_err());

    let mut exact_total = BoundedReader {
        inner: io::repeat(0).take(MAX_DECOMPRESSED_LAYER_BYTES),
        remaining: MAX_DECOMPRESSED_LAYER_BYTES,
        limit: MAX_DECOMPRESSED_LAYER_BYTES,
    };
    drain_decoder_to_eof(&mut exact_total).unwrap();
    let mut over_total = BoundedReader {
        inner: io::repeat(0).take(MAX_DECOMPRESSED_LAYER_BYTES + 1),
        remaining: MAX_DECOMPRESSED_LAYER_BYTES,
        limit: MAX_DECOMPRESSED_LAYER_BYTES,
    };
    assert!(drain_decoder_to_eof(&mut over_total).unwrap_err().contains("exceeds"));
}

#[test]
fn extraction_cap_bounds_whole_archive_scan() {
    let layer = build_custom_tar_gz(|builder| {
        let mut header = tar::Header::new_gnu();
        header.set_size(MAX_DECLARED_ENTRY_BYTES);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "filler.bin", io::repeat(0).take(MAX_DECLARED_ENTRY_BYTES))
            .unwrap();
    });
    let err = extract_wasm_from_tar_gz(&layer).unwrap_err();
    assert!(err.contains("decompressed layer exceeds"), "got: {err}");
}

#[test]
fn strictly_parses_pax_and_rejects_sparse_keys() {
    assert_eq!(parse_pax_payload(&pax_record("size", "8")).unwrap().first_size, Some(8));
    let duplicate = [pax_record("size", "8"), pax_record("size", "9")].concat();
    assert_eq!(parse_pax_payload(&duplicate).unwrap().first_size, Some(8));

    for malformed in [
        Vec::new(),
        b"x path=a\n".to_vec(),
        b"99 path=a\n".to_vec(),
        b"10 path=a".to_vec(),
        b"8 path\n".to_vec(),
        b"7 =x\n".to_vec(),
        b"184467440737095516160 size=1\n".to_vec(),
        pax_record("size", "-1"),
        [pax_record("path", "a"), b"junk".to_vec()].concat(),
        b"14 path=a\nb=c\n".to_vec(),
    ] {
        assert!(parse_pax_payload(&malformed).is_err(), "{malformed:?}");
    }
    assert!(parse_pax_payload(&pax_record("GNU.sparse.map", "0,1")).is_err());

    for entry_type in [tar::EntryType::XHeader, tar::EntryType::XGlobalHeader] {
        let sparse = build_custom_tar_gz(|builder| {
            append_pax(builder, &pax_record("GNU.sparse.major", "1"), entry_type);
            append_entry(builder, "plugin.wasm", b"wasm", tar::EntryType::Regular);
        });
        assert!(extract_wasm_from_tar_gz(&sparse).unwrap_err().contains("sparse PAX"));
    }
}

#[test]
fn pax_size_must_match_raw_header_and_survives_only_gnu_l_and_k() {
    let equal = build_custom_tar_gz(|builder| {
        let first_wins = [pax_record("size", "8"), pax_record("size", "9")].concat();
        append_pax(builder, &first_wins, tar::EntryType::XHeader);
        append_entry(builder, "plugin.wasm", b"\0asm1234", tar::EntryType::Regular);
    });
    assert_eq!(extract_wasm_from_tar_gz(&equal).unwrap(), b"\0asm1234");

    let across_long_headers = build_custom_tar_gz(|builder| {
        append_pax(builder, &pax_record("size", "8"), tar::EntryType::XHeader);
        append_entry(
            builder,
            "LongLink",
            b"nested/plugin.wasm\0",
            tar::EntryType::GNULongName,
        );
        append_entry(builder, "LongLink", b"unused-target\0", tar::EntryType::GNULongLink);
        append_entry(builder, "placeholder", b"\0asm1234", tar::EntryType::Regular);
    });
    assert_eq!(extract_wasm_from_tar_gz(&across_long_headers).unwrap(), b"\0asm1234");

    let global_after_local = build_custom_tar_gz(|builder| {
        append_pax(builder, &pax_record("comment", "local"), tar::EntryType::XHeader);
        append_pax(builder, &pax_record("comment", "global"), tar::EntryType::XGlobalHeader);
    });
    assert!(extract_wasm_from_tar_gz(&global_after_local)
        .unwrap_err()
        .contains("global PAX"));

    let orphan = build_custom_tar_gz(|builder| {
        append_pax(builder, &pax_record("size", "8"), tar::EntryType::XHeader);
    });
    assert!(extract_wasm_from_tar_gz(&orphan)
        .unwrap_err()
        .contains("no following tar entry"));

    let unknown_consumes_pending = build_custom_tar_gz(|builder| {
        append_pax(builder, &pax_record("size", "3"), tar::EntryType::XHeader);
        append_entry(builder, "vendor", b"abc", tar::EntryType::new(b'V'));
        append_entry(builder, "plugin.wasm", b"\0asm1234", tar::EntryType::Regular);
    });
    assert_eq!(
        extract_wasm_from_tar_gz(&unknown_consumes_pending).unwrap(),
        b"\0asm1234"
    );
}

#[test]
fn rejects_differing_pax_size_polyglot_and_old_gnu_sparse() {
    let polyglot = build_custom_tar_gz(|builder| {
        append_pax(builder, &pax_record("size", "512"), tar::EntryType::XHeader);
        let mut body = vec![0u8; 1024];
        let mut alternate = tar::Header::new_gnu();
        alternate.set_path("alternate").unwrap();
        alternate.set_size(0);
        alternate.set_mode(0o644);
        alternate.set_cksum();
        body[512..].copy_from_slice(alternate.as_bytes());
        append_entry(builder, "plugin.wasm", &body, tar::EntryType::Regular);
    });
    let error = extract_wasm_from_tar_gz(&polyglot).unwrap_err();
    assert!(error.contains("PAX size does not match"), "got: {error}");

    let sparse = build_custom_tar_gz(|builder| {
        append_entry(builder, "plugin.wasm", b"", tar::EntryType::GNUSparse);
    });
    assert!(extract_wasm_from_tar_gz(&sparse)
        .unwrap_err()
        .contains("old GNU sparse"));
}

#[test]
fn enforces_path_and_header_count_exact_boundaries() {
    for (length, succeeds) in [(MAX_ARCHIVE_PATH_BYTES, true), (MAX_ARCHIVE_PATH_BYTES + 1, false)] {
        let prefix = "a".repeat(length - "/plugin.wasm".len());
        let path = format!("{prefix}/plugin.wasm");
        let layer = build_custom_tar_gz(|builder| {
            append_pax(builder, &pax_record("path", &path), tar::EntryType::XHeader);
            append_entry(builder, "placeholder", b"wasm", tar::EntryType::Regular);
        });
        assert_eq!(extract_wasm_from_tar_gz(&layer).is_ok(), succeeds);
    }

    let exact = build_custom_tar_gz(|builder| {
        for _ in 0..MAX_ARCHIVE_HEADERS - 1 {
            append_entry(builder, "filler", b"", tar::EntryType::Regular);
        }
        append_entry(builder, "plugin.wasm", b"wasm", tar::EntryType::Regular);
    });
    assert_eq!(extract_wasm_from_tar_gz(&exact).unwrap(), b"wasm");

    let over = build_custom_tar_gz(|builder| {
        for _ in 0..MAX_ARCHIVE_HEADERS {
            append_entry(builder, "filler", b"", tar::EntryType::Regular);
        }
        append_entry(builder, "plugin.wasm", b"wasm", tar::EntryType::Regular);
    });
    assert!(extract_wasm_from_tar_gz(&over).unwrap_err().contains("header cap"));
}

fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn exact_cap_still_probes_decoder_crc() {
    let mut compressed = gzip_bytes(b"x");
    let crc_byte = compressed.len() - 8;
    compressed[crc_byte] ^= 1;
    let mut reader = BoundedReader {
        inner: flate2::read::MultiGzDecoder::new(compressed.as_slice()),
        remaining: 1,
        limit: 1,
    };
    assert!(drain_decoder_to_eof(&mut reader).is_err());
}

#[test]
fn freezes_candidates_at_first_tar_end_but_drains_later_members() {
    let mut first = build_tar_gz(&[("plugin.wasm", b"first")]);
    first.extend(build_tar_gz(&[("plugin.wasm", b"second")]));
    assert_eq!(extract_wasm_from_tar_gz(&first).unwrap(), b"first");

    let mut no_candidate = build_tar_gz(&[("other", b"x")]);
    no_candidate.extend(build_tar_gz(&[("plugin.wasm", b"later")]));
    assert!(extract_wasm_from_tar_gz(&no_candidate)
        .unwrap_err()
        .contains("no plugin.wasm"));
}

#[test]
fn malformed_later_member_fails_atomically_with_or_without_candidate() {
    let mut broken_member = build_tar_gz(&[("ignored", b"suffix")]);
    let crc_byte = broken_member.len() - 8;
    broken_member[crc_byte] ^= 1;

    for entries in [
        vec![("plugin.wasm", b"wasm".as_slice())],
        vec![("other", b"data".as_slice())],
    ] {
        let mut layer = build_tar_gz(&entries);
        layer.extend_from_slice(&broken_member);
        let error = extract_wasm_from_tar_gz(&layer).unwrap_err();
        assert!(error.contains("finish reading compressed layer"), "got: {error}");
        assert!(!error.contains("no plugin.wasm"), "got: {error}");
    }
}
