//! OCI manifest parsing, wasm layer selection, and compat-variant extraction.
//!
//! Two accepted image shapes (Istio parity):
//! - **oci variant** (solo.io wasm-image-spec): a layer whose media type is
//!   `application/vnd.module.wasm.content.layer.v1+wasm` holds the raw bytes.
//! - **compat variant**: a single docker/OCI tar+gzip layer (plain
//!   `docker build` from scratch) containing a `plugin.wasm` file.

use std::fmt;
use std::io::{self, Read};
use std::path::Component;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

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

const MANIFEST_TYPES: [&str; 2] = [
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
];

const MAX_OCI_DESCRIPTORS: usize = 4096;
const MAX_ARCHIVE_HEADERS: usize = 4096;
const MAX_ARCHIVE_PATH_BYTES: usize = 4096;
const MAX_ARCHIVE_EXTENSION_BYTES: u64 = 64 * 1024;
const MAX_DECLARED_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTRACTED_WASM_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECOMPRESSED_LAYER_BYTES: u64 = 64 * 1024 * 1024;

/// An OCI content descriptor (manifest `layers[]` / index `manifests[]` entry).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciDescriptor {
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(deserialize_with = "deserialize_digest")]
    pub digest: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// A validated superset of an image manifest and an image index. Exactly one
/// of `layers` / `manifests` is populated, including when `mediaType` is absent.
#[derive(Debug)]
pub struct OciManifest {
    pub media_type: Option<String>,
    pub layers: Vec<OciDescriptor>,
    pub manifests: Vec<OciDescriptor>,
}

fn deserialize_digest<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let digest = String::deserialize(deserializer)?;
    normalize_digest(&digest)
        .ok_or_else(|| de::Error::custom("descriptor digest must be sha256 followed by exactly 64 hex digits"))
}

fn normalize_digest(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("sha256:{}", hex.to_ascii_lowercase()))
}

struct DescriptorSeqSeed<'a> {
    remaining: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for DescriptorSeqSeed<'_> {
    type Value = Vec<OciDescriptor>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(DescriptorSeqVisitor {
            remaining: self.remaining,
        })
    }
}

struct DescriptorSeqVisitor<'a> {
    remaining: &'a mut usize,
}

impl<'de> Visitor<'de> for DescriptorSeqVisitor<'_> {
    type Value = Vec<OciDescriptor>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded OCI descriptor array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let capacity = seq.size_hint().unwrap_or(0).min(*self.remaining);
        let mut descriptors = Vec::new();
        descriptors
            .try_reserve_exact(capacity)
            .map_err(|_| de::Error::custom("allocate OCI descriptor list"))?;

        loop {
            if *self.remaining == 0 {
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(format_args!(
                        "manifest exceeds the {MAX_OCI_DESCRIPTORS}-descriptor limit"
                    )));
                }
                break;
            }

            descriptors
                .try_reserve(1)
                .map_err(|_| de::Error::custom("allocate OCI descriptor list"))?;
            match seq.next_element::<OciDescriptor>()? {
                Some(descriptor) => {
                    *self.remaining -= 1;
                    descriptors.push(descriptor);
                }
                None => break,
            }
        }
        Ok(descriptors)
    }
}

struct ManifestVisitor;

impl<'de> Visitor<'de> for ManifestVisitor {
    type Value = OciManifest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an OCI image manifest or image index")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut schema_version = None;
        let mut media_type = None;
        let mut layers = None;
        let mut manifests = None;
        let mut remaining = MAX_OCI_DESCRIPTORS;

        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "schemaVersion" => {
                    if schema_version.is_some() {
                        return Err(de::Error::duplicate_field("schemaVersion"));
                    }
                    schema_version = Some(map.next_value::<u64>()?);
                }
                "mediaType" => {
                    if media_type.is_some() {
                        return Err(de::Error::duplicate_field("mediaType"));
                    }
                    media_type = Some(map.next_value::<String>()?);
                }
                "layers" => {
                    if layers.is_some() {
                        return Err(de::Error::duplicate_field("layers"));
                    }
                    layers = Some(map.next_value_seed(DescriptorSeqSeed {
                        remaining: &mut remaining,
                    })?);
                }
                "manifests" => {
                    if manifests.is_some() {
                        return Err(de::Error::duplicate_field("manifests"));
                    }
                    manifests = Some(map.next_value_seed(DescriptorSeqSeed {
                        remaining: &mut remaining,
                    })?);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        if schema_version != Some(2) {
            return Err(de::Error::custom("manifest schemaVersion must be 2"));
        }

        let layers = layers.unwrap_or_default();
        let manifests = manifests.unwrap_or_default();
        let is_image = !layers.is_empty() && manifests.is_empty();
        let is_index = layers.is_empty() && !manifests.is_empty();
        if !is_image && !is_index {
            return Err(de::Error::custom(if layers.is_empty() {
                "manifest contains neither layers nor manifests"
            } else {
                "manifest cannot contain both layers and manifests"
            }));
        }

        if let Some(media_type) = media_type.as_deref() {
            let media_matches_shape = if is_image {
                MANIFEST_TYPES.contains(&media_type)
            } else {
                matches!(media_type, MEDIA_TYPE_OCI_INDEX | MEDIA_TYPE_DOCKER_MANIFEST_LIST)
            };
            if !media_matches_shape {
                return Err(de::Error::custom(
                    "manifest mediaType is unsupported or contradicts its descriptor shape",
                ));
            }
        }

        Ok(OciManifest {
            media_type,
            layers,
            manifests,
        })
    }
}

impl<'de> Deserialize<'de> for OciManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ManifestVisitor)
    }
}

/// Outcome of parsing manifest bytes.
pub enum ParsedManifest {
    Image(OciManifest),
    Index(Vec<OciDescriptor>),
}

pub fn parse_manifest(bytes: &[u8]) -> Result<ParsedManifest, String> {
    let m: OciManifest = serde_json::from_slice(bytes).map_err(|e| format!("parse manifest JSON: {e}"))?;
    if !m.manifests.is_empty() {
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
    let mut raw_layers = m
        .layers
        .iter()
        .filter(|layer| layer.media_type.as_deref() == Some(MEDIA_TYPE_WASM_LAYER));
    let raw_layer = raw_layers.next();
    if raw_layers.next().is_some() {
        return Err("image manifest contains multiple raw wasm layers".to_string());
    }
    if let Some(layer) = raw_layer {
        return Ok(WasmLayer::Raw(layer));
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

/// A reader that fails with a clear error once the underlying
/// (decompressing) reader has produced more than the allowed bytes.
struct BoundedReader<R> {
    inner: R,
    remaining: u64,
    limit: u64,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "decompressed layer exceeds the {}-byte cap",
                    self.limit
                ))),
            };
        }
        let max = buf.len().min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        let n64 = u64::try_from(n).map_err(|_| io::Error::other("reader length overflow"))?;
        self.remaining = self
            .remaining
            .checked_sub(n64)
            .ok_or_else(|| io::Error::other("reader length overflow"))?;
        Ok(n)
    }
}

#[derive(Default)]
struct PendingPax {
    first_size: Option<u64>,
}

fn parse_pax_payload(payload: &[u8]) -> Result<PendingPax, String> {
    if payload.is_empty() {
        return Err("malformed PAX extension".to_string());
    }

    let mut pending = PendingPax::default();
    let mut offset = 0usize;
    while offset < payload.len() {
        let relative_space = payload[offset..]
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| "malformed PAX extension".to_string())?;
        if relative_space == 0 {
            return Err("malformed PAX extension".to_string());
        }

        let mut record_len = 0usize;
        for byte in &payload[offset..offset + relative_space] {
            if !byte.is_ascii_digit() {
                return Err("malformed PAX extension".to_string());
            }
            record_len = record_len
                .checked_mul(10)
                .and_then(|value| value.checked_add(usize::from(*byte - b'0')))
                .ok_or_else(|| "malformed PAX extension".to_string())?;
        }
        let end = offset
            .checked_add(record_len)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| "malformed PAX extension".to_string())?;
        let value_start = offset
            .checked_add(relative_space + 1)
            .ok_or_else(|| "malformed PAX extension".to_string())?;
        if record_len <= relative_space + 2 || payload[end - 1] != b'\n' {
            return Err("malformed PAX extension".to_string());
        }
        let key_value = &payload[value_start..end - 1];
        if key_value.contains(&b'\n') {
            return Err("malformed PAX extension".to_string());
        }
        let equals = key_value
            .iter()
            .position(|byte| *byte == b'=')
            .filter(|position| *position > 0)
            .ok_or_else(|| "malformed PAX extension".to_string())?;
        let key = &key_value[..equals];
        let value = &key_value[equals + 1..];

        if key.starts_with(b"GNU.sparse.") {
            return Err("sparse PAX entries are not supported".to_string());
        }
        if key == b"size" {
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err("malformed PAX size".to_string());
            }
            let mut size = 0u64;
            for byte in value {
                size = size
                    .checked_mul(10)
                    .and_then(|current| current.checked_add(u64::from(*byte - b'0')))
                    .ok_or_else(|| "malformed PAX size".to_string())?;
            }
            if pending.first_size.is_none() {
                pending.first_size = Some(size);
            }
        }
        offset = end;
    }
    Ok(pending)
}

fn read_extension_payload<R: Read>(entry: &mut R, size: u64) -> Result<Vec<u8>, String> {
    if size > MAX_ARCHIVE_EXTENSION_BYTES {
        return Err(format!(
            "tar extension exceeds the {MAX_ARCHIVE_EXTENSION_BYTES}-byte cap"
        ));
    }
    let size = usize::try_from(size).map_err(|_| "tar extension size is unsupported")?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(size)
        .map_err(|_| "allocate tar extension payload".to_string())?;
    payload.resize(size, 0);
    entry
        .read_exact(&mut payload)
        .map_err(|_| "read tar extension payload".to_string())?;
    Ok(payload)
}

fn drain_extension<R: Read>(entry: &mut R, size: u64) -> Result<(), String> {
    if size > MAX_ARCHIVE_EXTENSION_BYTES {
        return Err(format!(
            "tar extension exceeds the {MAX_ARCHIVE_EXTENSION_BYTES}-byte cap"
        ));
    }
    let mut remaining = size;
    let mut buffer = [0u8; 8192];
    while remaining != 0 {
        let length = buffer.len().min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = entry
            .read(&mut buffer[..length])
            .map_err(|_| "read tar extension payload".to_string())?;
        if read == 0 {
            return Err("read tar extension payload".to_string());
        }
        remaining = remaining
            .checked_sub(u64::try_from(read).map_err(|_| "tar extension size overflow")?)
            .ok_or_else(|| "tar extension size overflow".to_string())?;
    }
    Ok(())
}

fn drain_decoder_to_eof<R: Read>(reader: &mut R) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(format!("finish reading compressed layer: {error}")),
        }
    }
}

fn new_layer_reader(bytes: &[u8]) -> BoundedReader<flate2::read::MultiGzDecoder<&[u8]>> {
    BoundedReader {
        inner: flate2::read::MultiGzDecoder::new(bytes),
        remaining: MAX_DECOMPRESSED_LAYER_BYTES,
        limit: MAX_DECOMPRESSED_LAYER_BYTES,
    }
}

fn validate_declared_entry_size(size: u64) -> Result<(), String> {
    if size > MAX_DECLARED_ENTRY_BYTES {
        return Err(format!(
            "tar entry exceeds the {MAX_DECLARED_ENTRY_BYTES}-byte declared-size cap"
        ));
    }
    Ok(())
}

fn preflight_tar_gz(bytes: &[u8]) -> Result<(), String> {
    let mut archive = tar::Archive::new(new_layer_reader(bytes));
    let mut header_count = 0usize;
    let mut pending_local_pax: Option<PendingPax> = None;
    {
        let mut entries = archive
            .entries()
            .map_err(|error| format!("read tar: {error}"))?
            .raw(true);
        for entry in &mut entries {
            let mut entry = entry.map_err(|error| format!("read tar entry: {error}"))?;
            header_count = header_count
                .checked_add(1)
                .ok_or_else(|| "tar header count overflow".to_string())?;
            if header_count > MAX_ARCHIVE_HEADERS {
                return Err(format!("tar archive exceeds the {MAX_ARCHIVE_HEADERS}-header cap"));
            }

            let raw_size = entry
                .header()
                .entry_size()
                .map_err(|error| format!("read tar entry size: {error}"))?;
            validate_declared_entry_size(raw_size)?;

            let entry_type = entry.header().entry_type();
            if entry_type.is_gnu_sparse() {
                return Err("old GNU sparse entries are not supported".to_string());
            }
            let recognized_header = entry.header().as_gnu().is_some() || entry.header().as_ustar().is_some();

            if recognized_header && (entry_type.is_gnu_longname() || entry_type.is_gnu_longlink()) {
                drain_extension(&mut entry, raw_size)?;
                continue;
            }
            if recognized_header && entry_type.is_pax_local_extensions() {
                if pending_local_pax.is_some() {
                    return Err("multiple local PAX headers describe one entry".to_string());
                }
                let payload = read_extension_payload(&mut entry, raw_size)?;
                pending_local_pax = Some(parse_pax_payload(&payload)?);
                continue;
            }
            if entry_type.is_pax_global_extensions() {
                if pending_local_pax.is_some() {
                    return Err("local PAX header cannot describe a global PAX entry".to_string());
                }
                let payload = read_extension_payload(&mut entry, raw_size)?;
                parse_pax_payload(&payload)?;
                continue;
            }

            if let Some(pending) = pending_local_pax.take() {
                if let Some(pax_size) = pending.first_size {
                    if pax_size != raw_size {
                        return Err("local PAX size does not match the following tar header".to_string());
                    }
                }
            }

            // An unrecognized-format `x` header is a logical entry to tar 0.4.46,
            // but its payload must still be strictly safe if it advertises PAX data.
            if entry_type.is_pax_local_extensions() {
                let payload = read_extension_payload(&mut entry, raw_size)?;
                parse_pax_payload(&payload)?;
            }
        }
    }
    if pending_local_pax.is_some() {
        return Err("local PAX header has no following tar entry".to_string());
    }
    let mut reader = archive.into_inner();
    drain_decoder_to_eof(&mut reader)
}

fn read_wasm_entry<R: Read>(entry: &mut R, declared_size: u64) -> Result<Vec<u8>, String> {
    if declared_size > MAX_EXTRACTED_WASM_BYTES {
        return Err(format!(
            "plugin.wasm exceeds the {MAX_EXTRACTED_WASM_BYTES}-byte output cap"
        ));
    }
    let size = usize::try_from(declared_size).map_err(|_| "plugin.wasm size is unsupported".to_string())?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| "allocate plugin.wasm output".to_string())?;
    output.resize(size, 0);
    entry
        .read_exact(&mut output)
        .map_err(|_| "read plugin.wasm from layer".to_string())?;
    Ok(output)
}

/// Unpack `plugin.wasm` from a compat-variant tar+gzip layer.
pub fn extract_wasm_from_tar_gz(bytes: &[u8]) -> Result<Vec<u8>, String> {
    preflight_tar_gz(bytes)?;

    let mut archive = tar::Archive::new(new_layer_reader(bytes));
    let mut logical_entries = 0usize;
    let mut output = None;
    {
        let entries = archive.entries().map_err(|error| format!("read tar: {error}"))?;
        for entry in entries {
            let mut entry = entry.map_err(|error| format!("read tar entry: {error}"))?;
            logical_entries = logical_entries
                .checked_add(1)
                .ok_or_else(|| "tar entry count overflow".to_string())?;
            if logical_entries > MAX_ARCHIVE_HEADERS {
                return Err(format!("tar archive exceeds the {MAX_ARCHIVE_HEADERS}-entry cap"));
            }

            let declared_size = entry.size();
            validate_declared_entry_size(declared_size)?;

            let path_bytes = entry.path_bytes();
            if path_bytes.len() > MAX_ARCHIVE_PATH_BYTES {
                return Err(format!("tar path exceeds the {MAX_ARCHIVE_PATH_BYTES}-byte cap"));
            }
            let path = entry.path().map_err(|_| "read tar entry path".to_string())?;
            let is_plugin = path.file_name().map(|name| name == "plugin.wasm").unwrap_or(false);
            if !is_plugin {
                continue;
            }

            let safe_relative_path = !path.is_absolute()
                && path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
            if !safe_relative_path {
                return Err("plugin.wasm has an unsafe archive path".to_string());
            }
            if !entry.header().entry_type().is_file() {
                return Err("plugin.wasm must be a regular file".to_string());
            }
            if output.is_some() {
                return Err("compat image layer contains multiple plugin.wasm entries".to_string());
            }
            output = Some(read_wasm_entry(&mut entry, declared_size)?);
        }
    }
    let mut reader = archive.into_inner();
    drain_decoder_to_eof(&mut reader)?;
    output.ok_or_else(|| "compat image layer contains no plugin.wasm entry".to_string())
}

#[cfg(test)]
mod tests {
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
}
