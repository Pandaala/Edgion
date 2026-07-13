//! OCI manifest parsing, wasm layer selection, and compat-variant extraction.
//!
//! Two accepted image shapes (Istio parity):
//! - **oci variant** (solo.io wasm-image-spec): a layer whose media type is
//!   `application/vnd.module.wasm.content.layer.v1+wasm` holds the raw bytes.
//! - **compat variant**: a single docker/OCI tar+gzip layer (plain
//!   `docker build` from scratch) containing a `plugin.wasm` file.

use serde::Deserialize;

pub const MEDIA_TYPE_WASM_LAYER: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
pub const MEDIA_TYPE_OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const MEDIA_TYPE_DOCKER_MANIFEST_LIST: &str = "application/vnd.docker.distribution.manifest.list.v2+json";

/// `Accept` for manifest requests: single manifests and indexes, OCI and
/// Docker vocabularies alike.
pub const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json";

const TAR_GZ_LAYER_TYPES: [&str; 2] = [
    "application/vnd.oci.image.layer.v1.tar+gzip",
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
];

/// An OCI content descriptor (manifest `layers[]` / index `manifests[]` entry).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciDescriptor {
    #[serde(default)]
    pub media_type: Option<String>,
    pub digest: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// A permissive superset of an image manifest and an image index: whichever
/// of `layers` / `manifests` is populated tells the two shapes apart when
/// `mediaType` is absent.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciManifest {
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub layers: Vec<OciDescriptor>,
    #[serde(default)]
    pub manifests: Vec<OciDescriptor>,
}

/// Outcome of parsing manifest bytes.
pub enum ParsedManifest {
    Image(OciManifest),
    Index(Vec<OciDescriptor>),
}

pub fn parse_manifest(bytes: &[u8]) -> Result<ParsedManifest, String> {
    let m: OciManifest = serde_json::from_slice(bytes).map_err(|e| format!("parse manifest JSON: {e}"))?;
    let is_index = matches!(
        m.media_type.as_deref(),
        Some(MEDIA_TYPE_OCI_INDEX) | Some(MEDIA_TYPE_DOCKER_MANIFEST_LIST)
    ) || (!m.manifests.is_empty() && m.layers.is_empty());
    if is_index {
        if m.manifests.is_empty() {
            return Err("image index contains no manifests".to_string());
        }
        Ok(ParsedManifest::Index(m.manifests))
    } else {
        Ok(ParsedManifest::Image(m))
    }
}

/// The wasm-bearing layer of an image manifest.
#[derive(Debug)]
pub enum WasmLayer<'a> {
    /// Raw wasm bytes (oci variant).
    Raw(&'a OciDescriptor),
    /// tar+gzip layer containing `plugin.wasm` (compat variant).
    TarGz(&'a OciDescriptor),
}

pub fn select_wasm_layer(m: &OciManifest) -> Result<WasmLayer<'_>, String> {
    if let Some(l) = m
        .layers
        .iter()
        .find(|l| l.media_type.as_deref() == Some(MEDIA_TYPE_WASM_LAYER))
    {
        return Ok(WasmLayer::Raw(l));
    }
    if m.layers.len() == 1 {
        let l = &m.layers[0];
        if l.media_type
            .as_deref()
            .map(|t| TAR_GZ_LAYER_TYPES.contains(&t))
            .unwrap_or(false)
        {
            return Ok(WasmLayer::TarGz(l));
        }
    }
    Err(format!(
        "no wasm layer found: expected a layer with media type '{MEDIA_TYPE_WASM_LAYER}' \
         (oci variant) or a single tar+gzip layer containing plugin.wasm \
         (compat variant); got {} layer(s)",
        m.layers.len()
    ))
}

/// Cap on the total decompressed size of a compat layer. The cap must cover
/// the WHOLE archive stream, not just the matched entry: skipping past
/// non-matching entries decompresses them too, so a hostile archive could
/// otherwise expand far beyond its compressed size during the scan.
const MAX_EXTRACTED_WASM_BYTES: u64 = 64 * 1024 * 1024;

/// A reader that fails with a clear error once the underlying
/// (decompressing) reader has produced more than the allowed bytes.
struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: std::io::Read> std::io::Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other(format!(
                "decompressed layer exceeds the {MAX_EXTRACTED_WASM_BYTES}-byte extraction cap"
            )));
        }
        let max = buf.len().min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Unpack `plugin.wasm` from a compat-variant tar+gzip layer.
pub fn extract_wasm_from_tar_gz(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let gz = BoundedReader {
        inner: flate2::read::GzDecoder::new(bytes),
        remaining: MAX_EXTRACTED_WASM_BYTES,
    };
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().map_err(|e| format!("read tar: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read tar entry: {e}"))?;
        let is_plugin = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "plugin.wasm"))
            .unwrap_or(false);
        if is_plugin {
            let mut out = Vec::new();
            entry
                .read_to_end(&mut out)
                .map_err(|e| format!("read plugin.wasm from layer: {e}"))?;
            return Ok(out);
        }
    }
    Err("compat image layer contains no plugin.wasm entry".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let enc = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
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
            {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": "sha256:aa", "size": 8}
        ]));
        assert!(matches!(parse_manifest(&img).unwrap(), ParsedManifest::Image(_)));

        let idx = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": MEDIA_TYPE_OCI_INDEX,
            "manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": "sha256:bb"}]
        }))
        .unwrap();
        match parse_manifest(&idx).unwrap() {
            ParsedManifest::Index(entries) => assert_eq!(entries[0].digest, "sha256:bb"),
            ParsedManifest::Image(_) => panic!("expected index"),
        }
        assert!(parse_manifest(b"{not json").is_err());
    }

    #[test]
    fn selects_wasm_layer_preferring_media_type() {
        let m: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
            {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:aa"},
            {"mediaType": MEDIA_TYPE_WASM_LAYER, "digest": "sha256:bb"}
        ])))
        .unwrap();
        match select_wasm_layer(&m).unwrap() {
            WasmLayer::Raw(d) => assert_eq!(d.digest, "sha256:bb"),
            WasmLayer::TarGz(_) => panic!("expected raw wasm layer"),
        }
    }

    #[test]
    fn single_targz_layer_is_compat_and_multi_layer_without_wasm_errors() {
        let single: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
            {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": "sha256:aa"}
        ])))
        .unwrap();
        assert!(matches!(select_wasm_layer(&single).unwrap(), WasmLayer::TarGz(_)));

        let multi: OciManifest = serde_json::from_slice(&manifest_json(serde_json::json!([
            {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": "sha256:aa"},
            {"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": "sha256:bb"}
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
    fn extraction_cap_bounds_whole_archive_scan() {
        // A highly-compressible 65MiB non-matching entry before plugin.wasm:
        // the scan itself must hit the cap, not just the matched entry.
        let big = vec![0u8; 65 * 1024 * 1024];
        let layer = build_tar_gz(&[("filler.bin", big.as_slice()), ("plugin.wasm", b"\0asm")]);
        let err = extract_wasm_from_tar_gz(&layer).unwrap_err();
        assert!(err.contains("extraction cap"), "got: {err}");
    }
}
