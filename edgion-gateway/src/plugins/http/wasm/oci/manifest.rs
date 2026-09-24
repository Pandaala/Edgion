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
#[path = "../../../../../tests/unit/plugins/http/wasm/oci/manifest_test.rs"]
mod tests;
