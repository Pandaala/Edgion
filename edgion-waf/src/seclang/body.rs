use memchr::memmem::Finder;
use thiserror::Error;

use super::CollectionKind;

pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_BODY_ENTRIES: usize = 65_536;
pub const DEFAULT_MAX_BODY_DECODED_BYTES: usize = 32 * 1024 * 1024;
pub const DEFAULT_MAX_BODY_NAME_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_BODY_DEPTH: usize = 64;
pub const DEFAULT_MAX_MULTIPART_PARTS: usize = 4_096;
pub const DEFAULT_MAX_MULTIPART_HEADERS: usize = 16_384;
pub const DEFAULT_MAX_BODY_WORK_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    pub max_body_bytes: usize,
    pub max_entries: usize,
    pub max_decoded_bytes: usize,
    pub max_name_bytes: usize,
    pub max_depth: usize,
    pub max_multipart_parts: usize,
    pub max_multipart_headers: usize,
    pub max_work_bytes: usize,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            max_entries: DEFAULT_MAX_BODY_ENTRIES,
            max_decoded_bytes: DEFAULT_MAX_BODY_DECODED_BYTES,
            max_name_bytes: DEFAULT_MAX_BODY_NAME_BYTES,
            max_depth: DEFAULT_MAX_BODY_DEPTH,
            max_multipart_parts: DEFAULT_MAX_MULTIPART_PARTS,
            max_multipart_headers: DEFAULT_MAX_MULTIPART_HEADERS,
            max_work_bytes: DEFAULT_MAX_BODY_WORK_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyProcessorSelection {
    Auto,
    Raw,
    UrlEncoded,
    Json,
    Multipart,
    Xml,
}

impl BodyProcessorSelection {
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Auto | Self::Raw => b"RAW",
            Self::UrlEncoded => b"URLENCODED",
            Self::Json => b"JSON",
            Self::Multipart => b"MULTIPART",
            Self::Xml => b"XML",
        }
    }
}

/// One complete owned source. Cloning shares its bytes without copying the body.
#[derive(Clone)]
pub struct RequestBody {
    bytes: bytes::Bytes,
    processor: BodyProcessorSelection,
}

impl std::fmt::Debug for RequestBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestBody")
            .field("bytes", &self.bytes.len())
            .field("processor", &self.processor)
            .finish()
    }
}

impl RequestBody {
    pub const fn new(bytes: bytes::Bytes, processor: BodyProcessorSelection) -> Self {
        Self { bytes, processor }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_parts(self) -> (bytes::Bytes, BodyProcessorSelection) {
        (self.bytes, self.processor)
    }

    pub const fn processor(&self) -> BodyProcessorSelection {
        self.processor
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    #[error("request body has {actual} bytes, exceeding the limit of {limit}")]
    ByteLimit { actual: usize, limit: usize },
    #[error("request body derived entries exceed the limit of {limit}")]
    EntryLimit { limit: usize },
    #[error("request body decoded bytes exceed the limit of {limit}")]
    DecodedByteLimit { limit: usize },
    #[error("request body field name exceeds the limit of {limit}")]
    NameByteLimit { limit: usize },
    #[error("request body nesting exceeds the limit of {limit}")]
    DepthLimit { limit: usize },
    #[error("multipart parts exceed the limit of {limit}")]
    MultipartPartLimit { limit: usize },
    #[error("multipart headers exceed the limit of {limit}")]
    MultipartHeaderLimit { limit: usize },
    #[error("request body parser work exceeds the limit of {limit}")]
    WorkLimit { limit: usize },
    #[error("request body content type does not provide a valid multipart boundary")]
    MultipartBoundary,
    #[error("request body is malformed for the selected {processor:?} processor")]
    Malformed { processor: BodyProcessorSelection },
    #[error("cannot allocate bounded request body state")]
    Allocation,
    #[error("request body accounting arithmetic overflowed")]
    Arithmetic,
}

pub(super) struct BodyEntry {
    pub collection: CollectionKind,
    pub name: Option<Vec<u8>>,
    pub value: Vec<u8>,
    pub xml_attribute: bool,
}

pub(super) struct BodyPublication {
    pub entries: Vec<BodyEntry>,
    pub retained_bytes: usize,
}

pub(super) fn build_publication(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
) -> Result<BodyPublication, BodyError> {
    build_publication_with_raw_policy(body, content_type, head_args_combined_size, limits, None)
}

pub(super) fn build_publication_with_raw_policy(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
    force_raw_body: Option<bool>,
) -> Result<BodyPublication, BodyError> {
    build_publication_inner(
        body,
        content_type,
        head_args_combined_size,
        limits,
        true,
        force_raw_body,
    )
}

pub(super) fn build_fallback_publication(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
) -> Result<BodyPublication, BodyError> {
    build_fallback_publication_with_raw_policy(body, content_type, head_args_combined_size, limits, None)
}

pub(super) fn build_fallback_publication_with_raw_policy(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
    force_raw_body: Option<bool>,
) -> Result<BodyPublication, BodyError> {
    build_publication_inner(
        body,
        content_type,
        head_args_combined_size,
        limits,
        false,
        force_raw_body,
    )
}

fn build_publication_inner(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
    parse_derived_views: bool,
    force_raw_body: Option<bool>,
) -> Result<BodyPublication, BodyError> {
    check_body_bytes(body.bytes.len(), limits)?;
    let processor = if body.processor == BodyProcessorSelection::Auto {
        processor_from_content_type(content_type)
    } else {
        body.processor
    };
    let mut builder = BodyBuilder::new(limits);
    builder.charge_work(body.bytes.len())?;
    if force_raw_body.unwrap_or(
        !parse_derived_views
            || matches!(
                processor,
                BodyProcessorSelection::Auto | BodyProcessorSelection::Raw | BodyProcessorSelection::UrlEncoded
            ),
    ) {
        builder.push(CollectionKind::RequestBody, None, &body.bytes, false)?;
    }
    let length = body.bytes.len().to_string();
    builder.push(CollectionKind::RequestBodyLength, None, length.as_bytes(), false)?;
    builder.push(CollectionKind::ReqbodyProcessor, None, processor.as_bytes(), false)?;
    if parse_derived_views && !body.bytes.is_empty() {
        match processor {
            BodyProcessorSelection::Auto | BodyProcessorSelection::Raw => {}
            BodyProcessorSelection::UrlEncoded => parse_urlencoded(&body.bytes, &mut builder)?,
            BodyProcessorSelection::Json => return Err(BodyError::Malformed { processor }),
            BodyProcessorSelection::Multipart => parse_multipart(&body.bytes, content_type, &mut builder)?,
            BodyProcessorSelection::Xml => parse_xml(&body.bytes, &mut builder)?,
        }
    }
    let combined_size = builder.accounting.combined_size(head_args_combined_size)?.to_string();
    builder.push(CollectionKind::ArgsCombinedSize, None, combined_size.as_bytes(), false)?;
    builder.finish()
}

pub(super) fn processor_from_content_type(content_type: Option<&[u8]>) -> BodyProcessorSelection {
    let Some(content_type) = content_type else {
        return BodyProcessorSelection::Raw;
    };
    let media_type = content_type
        .split(|byte| *byte == b';')
        .next()
        .map(trim_ascii)
        .unwrap_or_default();
    if media_type.eq_ignore_ascii_case(b"application/x-www-form-urlencoded") {
        BodyProcessorSelection::UrlEncoded
    } else if media_type.eq_ignore_ascii_case(b"application/json") || ends_with_ascii_case(media_type, b"+json") {
        BodyProcessorSelection::Json
    } else if media_type.eq_ignore_ascii_case(b"multipart/form-data") {
        BodyProcessorSelection::Multipart
    } else if media_type.eq_ignore_ascii_case(b"application/xml")
        || media_type.eq_ignore_ascii_case(b"text/xml")
        || ends_with_ascii_case(media_type, b"+xml")
    {
        BodyProcessorSelection::Xml
    } else {
        BodyProcessorSelection::Raw
    }
}

fn ends_with_ascii_case(value: &[u8], suffix: &[u8]) -> bool {
    value
        .len()
        .checked_sub(suffix.len())
        .and_then(|start| value.get(start..))
        .is_some_and(|ending| ending.eq_ignore_ascii_case(suffix))
}

/// Logical complete-body publication costs, not the reducer's allocation usage.
/// `retained_bytes` includes the materialized entry descriptors and payload
/// lengths; this validator does not allocate or retain those payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonPublicationSummary {
    /// Includes optional raw bytes, processor/length metadata, and final combined size.
    pub entries: usize,
    /// Sum of published name and value lengths, including repeated ARGS_NAMES bytes.
    pub decoded_bytes: usize,
    /// Source bytes plus surviving JSON visit path lengths; excludes lexical/reducer work.
    pub work_bytes: usize,
    /// Surviving body ARGS name and value bytes, excluding ARGS_NAMES copies.
    pub body_args_combined_size: usize,
    /// Head ARGS bytes plus `body_args_combined_size`.
    pub args_combined_size: usize,
    /// Decoded bytes plus logical materialized entry descriptors, not allocated capacity.
    pub retained_bytes: usize,
}

fn decimal_bytes(value: usize) -> usize {
    value.checked_ilog10().map_or(1, |digits| digits as usize + 1)
}

fn check_body_bytes(actual: usize, limits: BodyLimits) -> Result<(), BodyError> {
    if actual > limits.max_body_bytes {
        return Err(BodyError::ByteLimit {
            actual,
            limit: limits.max_body_bytes,
        });
    }
    Ok(())
}

fn check_name_bytes(actual: usize, limit: usize) -> Result<(), BodyError> {
    if actual > limit {
        return Err(BodyError::NameByteLimit { limit });
    }
    Ok(())
}

pub(super) struct BodyAccounting {
    limits: BodyLimits,
    entries: usize,
    decoded_bytes: usize,
    work_bytes: usize,
    body_args_size: usize,
}

impl BodyAccounting {
    pub(super) fn json_prelude(
        source_bytes: usize,
        limits: BodyLimits,
        retain_raw_body: bool,
    ) -> Result<Self, BodyError> {
        check_body_bytes(source_bytes, limits)?;
        let mut accounting = Self::new(limits);
        accounting.charge_work(source_bytes)?;
        if retain_raw_body {
            accounting.push(CollectionKind::RequestBody, None, source_bytes)?;
        }
        accounting.push(CollectionKind::RequestBodyLength, None, decimal_bytes(source_bytes))?;
        accounting.push(
            CollectionKind::ReqbodyProcessor,
            None,
            BodyProcessorSelection::Json.as_bytes().len(),
        )?;
        Ok(accounting)
    }

    /// Fixed final priority replaces the DOM walk's first failing node: names,
    /// depth, entries, decoded bytes, traversal work, then final metadata.
    pub(super) fn apply_json_summary(&mut self, summary: super::JsonSemanticSummary) -> Result<(), BodyError> {
        check_name_bytes(summary.max_checked_name_bytes, self.limits.max_name_bytes)?;
        if summary.max_value_depth > self.limits.max_depth {
            return Err(BodyError::DepthLimit {
                limit: self.limits.max_depth,
            });
        }
        self.entries = self
            .entries
            .checked_add(summary.body_entry_count)
            .ok_or(BodyError::Arithmetic)?;
        if self.entries > self.limits.max_entries {
            return Err(BodyError::EntryLimit {
                limit: self.limits.max_entries,
            });
        }
        self.decoded_bytes = self
            .decoded_bytes
            .checked_add(summary.decoded_bytes)
            .ok_or(BodyError::Arithmetic)?;
        if self.decoded_bytes > self.limits.max_decoded_bytes {
            return Err(BodyError::DecodedByteLimit {
                limit: self.limits.max_decoded_bytes,
            });
        }
        self.charge_work(summary.traversal_work)?;
        self.body_args_size = summary.combined_size;
        Ok(())
    }

    pub(super) fn finish_json(mut self, head_args_combined_size: usize) -> Result<JsonPublicationSummary, BodyError> {
        let args_combined_size = self.combined_size(head_args_combined_size)?;
        self.push(
            CollectionKind::ArgsCombinedSize,
            None,
            decimal_bytes(args_combined_size),
        )?;
        Ok(JsonPublicationSummary {
            entries: self.entries,
            decoded_bytes: self.decoded_bytes,
            work_bytes: self.work_bytes,
            body_args_combined_size: self.body_args_size,
            args_combined_size,
            retained_bytes: self.retained_bytes()?,
        })
    }

    fn new(limits: BodyLimits) -> Self {
        Self {
            limits,
            entries: 0,
            decoded_bytes: 0,
            work_bytes: 0,
            body_args_size: 0,
        }
    }

    fn charge_work(&mut self, bytes: usize) -> Result<(), BodyError> {
        self.work_bytes = self.work_bytes.checked_add(bytes).ok_or(BodyError::Arithmetic)?;
        if self.work_bytes > self.limits.max_work_bytes {
            return Err(BodyError::WorkLimit {
                limit: self.limits.max_work_bytes,
            });
        }
        Ok(())
    }

    fn push(
        &mut self,
        collection: CollectionKind,
        name_bytes: Option<usize>,
        value_bytes: usize,
    ) -> Result<(), BodyError> {
        if self.entries == self.limits.max_entries {
            return Err(BodyError::EntryLimit {
                limit: self.limits.max_entries,
            });
        }
        if let Some(name_bytes) = name_bytes {
            check_name_bytes(name_bytes, self.limits.max_name_bytes)?;
        }
        let added = name_bytes
            .unwrap_or(0)
            .checked_add(value_bytes)
            .ok_or(BodyError::Arithmetic)?;
        self.decoded_bytes = self.decoded_bytes.checked_add(added).ok_or(BodyError::Arithmetic)?;
        if self.decoded_bytes > self.limits.max_decoded_bytes {
            return Err(BodyError::DecodedByteLimit {
                limit: self.limits.max_decoded_bytes,
            });
        }
        // ARGS bytes are a subset of the successfully checked decoded bytes.
        // Only combining them with head ARGS can newly overflow at finish time.
        if collection == CollectionKind::Args {
            self.body_args_size = self.body_args_size.checked_add(added).ok_or(BodyError::Arithmetic)?;
        }
        self.entries = self.entries.checked_add(1).ok_or(BodyError::Arithmetic)?;
        Ok(())
    }

    fn combined_size(&self, head_args_combined_size: usize) -> Result<usize, BodyError> {
        head_args_combined_size
            .checked_add(self.body_args_size)
            .ok_or(BodyError::Arithmetic)
    }

    fn retained_bytes(&self) -> Result<usize, BodyError> {
        let descriptors = self
            .entries
            .checked_mul(std::mem::size_of::<BodyEntry>())
            .ok_or(BodyError::Arithmetic)?;
        self.decoded_bytes.checked_add(descriptors).ok_or(BodyError::Arithmetic)
    }
}

struct BodyBuilder {
    accounting: BodyAccounting,
    entries: Vec<BodyEntry>,
    multipart_parts: usize,
    multipart_headers: usize,
}

impl BodyBuilder {
    fn new(limits: BodyLimits) -> Self {
        Self {
            accounting: BodyAccounting::new(limits),
            entries: Vec::new(),
            multipart_parts: 0,
            multipart_headers: 0,
        }
    }

    fn charge_work(&mut self, bytes: usize) -> Result<(), BodyError> {
        self.accounting.charge_work(bytes)
    }

    fn push(
        &mut self,
        collection: CollectionKind,
        name: Option<&[u8]>,
        value: &[u8],
        xml_attribute: bool,
    ) -> Result<(), BodyError> {
        self.accounting.push(collection, name.map(<[u8]>::len), value.len())?;
        self.entries.try_reserve(1).map_err(|_| BodyError::Allocation)?;
        self.entries.push(BodyEntry {
            collection,
            name: name.map(try_copy).transpose()?,
            value: try_copy(value)?,
            xml_attribute,
        });
        Ok(())
    }

    fn finish(self) -> Result<BodyPublication, BodyError> {
        Ok(BodyPublication {
            retained_bytes: self.accounting.retained_bytes()?,
            entries: self.entries,
        })
    }

    fn add_multipart_part(&mut self) -> Result<(), BodyError> {
        self.multipart_parts = self.multipart_parts.checked_add(1).ok_or(BodyError::Arithmetic)?;
        if self.multipart_parts > self.accounting.limits.max_multipart_parts {
            return Err(BodyError::MultipartPartLimit {
                limit: self.accounting.limits.max_multipart_parts,
            });
        }
        Ok(())
    }

    fn add_multipart_header(&mut self) -> Result<(), BodyError> {
        self.multipart_headers = self.multipart_headers.checked_add(1).ok_or(BodyError::Arithmetic)?;
        if self.multipart_headers > self.accounting.limits.max_multipart_headers {
            return Err(BodyError::MultipartHeaderLimit {
                limit: self.accounting.limits.max_multipart_headers,
            });
        }
        Ok(())
    }
}

fn parse_urlencoded(input: &[u8], builder: &mut BodyBuilder) -> Result<(), BodyError> {
    for segment in input.split(|byte| *byte == b'&').filter(|segment| !segment.is_empty()) {
        builder.charge_work(segment.len())?;
        let (name, value) = split_once(segment, b'=');
        let name = percent_decode(name)?;
        let value = percent_decode(value)?;
        builder.push(CollectionKind::Args, Some(&name), &value, false)?;
        builder.push(CollectionKind::ArgsNames, Some(&name), &name, false)?;
    }
    Ok(())
}

fn parse_multipart(input: &[u8], content_type: Option<&[u8]>, builder: &mut BodyBuilder) -> Result<(), BodyError> {
    let boundary = multipart_boundary(content_type).ok_or(BodyError::MultipartBoundary)?;
    if boundary.is_empty() || boundary.len() > 200 {
        return Err(BodyError::MultipartBoundary);
    }
    let mut delimiter_bytes = [0_u8; 202];
    delimiter_bytes[..2].copy_from_slice(b"--");
    delimiter_bytes[2..boundary.len() + 2].copy_from_slice(boundary);
    let delimiter = &delimiter_bytes[..boundary.len() + 2];
    let mut line_delimiter = [0_u8; 204];
    line_delimiter[..2].copy_from_slice(b"\r\n");
    line_delimiter[2..delimiter.len() + 2].copy_from_slice(delimiter);
    let searcher = if boundary.contains(&b'\r') || boundary.contains(&b'\n') {
        MultipartBoundarySearcher::Overlapping(multipart_prefix_table(delimiter))
    } else {
        MultipartBoundarySearcher::Line(Finder::new(&line_delimiter[..delimiter.len() + 2]))
    };
    let mut combined_size = 0_usize;
    let mut cursor = 0_usize;
    while let Some(boundary_start) = find_multipart_boundary(input, delimiter, &searcher, cursor) {
        let after = boundary_start
            .checked_add(delimiter.len())
            .ok_or(BodyError::Arithmetic)?;
        if input.get(after..after + 2) == Some(b"--") {
            let size = combined_size.to_string();
            builder.push(CollectionKind::FilesCombinedSize, None, size.as_bytes(), false)?;
            return Ok(());
        }
        let part_start = if input.get(after..after + 2) == Some(b"\r\n") {
            after + 2
        } else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Multipart,
            });
        };
        let Some(next_boundary) = find_multipart_boundary(input, delimiter, &searcher, part_start) else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Multipart,
            });
        };
        let mut part_end = next_boundary;
        if input.get(part_end.saturating_sub(2)..part_end) == Some(b"\r\n") {
            part_end -= 2;
        }
        builder.add_multipart_part()?;
        let content_size = parse_multipart_part(&input[part_start..part_end], builder)?;
        combined_size = combined_size.checked_add(content_size).ok_or(BodyError::Arithmetic)?;
        cursor = next_boundary;
    }
    Err(BodyError::Malformed {
        processor: BodyProcessorSelection::Multipart,
    })
}

fn parse_multipart_part(part: &[u8], builder: &mut BodyBuilder) -> Result<usize, BodyError> {
    builder.charge_work(part.len())?;
    let Some(separator) = find_bytes(part, b"\r\n\r\n") else {
        return Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Multipart,
        });
    };
    let headers = &part[..separator];
    let content = &part[separator + 4..];
    let mut field_name = None;
    let mut filename = None;
    for line in headers.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line[0].is_ascii_whitespace() {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Multipart,
            });
        }
        builder.add_multipart_header()?;
        let (name, value) = split_once(line, b':');
        if value.is_empty() || name.is_empty() || !name.iter().copied().all(is_header_name_byte) {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Multipart,
            });
        }
        if name.eq_ignore_ascii_case(b"content-disposition") {
            field_name = disposition_parameter(value, b"name");
            filename = disposition_parameter(value, b"filename");
        }
    }
    let field_name = field_name.ok_or(BodyError::Malformed {
        processor: BodyProcessorSelection::Multipart,
    })?;
    for line in headers.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        builder.push(CollectionKind::MultipartPartHeaders, Some(field_name), line, false)?;
    }
    if let Some(filename) = filename {
        builder.push(CollectionKind::Files, None, filename, false)?;
        builder.push(CollectionKind::FilesNames, None, field_name, false)?;
    } else {
        builder.push(CollectionKind::Args, Some(field_name), content, false)?;
        builder.push(CollectionKind::ArgsNames, Some(field_name), field_name, false)?;
    }
    Ok(content.len())
}

enum MultipartBoundarySearcher<'a> {
    // Without CR/LF inside the boundary, complete line delimiters cannot overlap.
    Line(Finder<'a>),
    // The accepted boundary syntax also permits overlapping malformed values.
    Overlapping([u8; 202]),
}

fn multipart_prefix_table(delimiter: &[u8]) -> [u8; 202] {
    let mut prefix = [0; 202];
    let mut matched = 0;
    for index in 1..delimiter.len() {
        while matched > 0 && delimiter[index] != delimiter[matched] {
            matched = usize::from(prefix[matched - 1]);
        }
        if delimiter[index] == delimiter[matched] {
            matched += 1;
        }
        // The delimiter is at most 202 bytes, so each proper prefix fits in u8.
        prefix[index] = matched as u8;
    }
    prefix
}

fn find_multipart_boundary(
    input: &[u8],
    delimiter: &[u8],
    searcher: &MultipartBoundarySearcher<'_>,
    cursor: usize,
) -> Option<usize> {
    let valid_suffix = |found: usize| {
        let after = found + delimiter.len();
        input.get(after..after + 2) == Some(b"\r\n") || input.get(after..after + 2) == Some(b"--")
    };
    match searcher {
        MultipartBoundarySearcher::Line(finder) => {
            if cursor == 0 && input.starts_with(delimiter) && valid_suffix(0) {
                return Some(0);
            }
            // Cursor bounds the "--boundary" start; include its preceding CRLF.
            let scan_start = cursor.saturating_sub(2);
            for relative in finder.find_iter(input.get(scan_start..)?) {
                let found = scan_start + relative + 2;
                if found >= cursor && valid_suffix(found) {
                    return Some(found);
                }
            }
        }
        MultipartBoundarySearcher::Overlapping(prefix) => {
            let mut matched = 0;
            for (relative, &byte) in input.get(cursor..)?.iter().enumerate() {
                while matched > 0 && byte != delimiter[matched] {
                    matched = usize::from(prefix[matched - 1]);
                }
                if byte == delimiter[matched] {
                    matched += 1;
                }
                if matched == delimiter.len() {
                    let found = cursor + relative + 1 - delimiter.len();
                    let starts_line = found == 0 || input.get(found.saturating_sub(2)..found) == Some(b"\r\n");
                    if starts_line && valid_suffix(found) {
                        return Some(found);
                    }
                    matched = usize::from(prefix[matched - 1]);
                }
            }
        }
    }
    None
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn disposition_parameter<'input>(value: &'input [u8], expected: &[u8]) -> Option<&'input [u8]> {
    let mut cursor = value.iter().position(|byte| *byte == b';')?.checked_add(1)?;
    while cursor <= value.len() {
        let start = cursor;
        let mut quoted = false;
        let mut escaped = false;
        while cursor < value.len() {
            let byte = value[cursor];
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if byte == b';' && !quoted {
                break;
            }
            cursor += 1;
        }
        let parameter = &value[start..cursor];
        let (name, value) = split_once(trim_ascii(parameter), b'=');
        if name.eq_ignore_ascii_case(expected) {
            let value = trim_ascii(value);
            return if value.len() >= 2 && value.first() == Some(&b'"') && value.last() == Some(&b'"') {
                Some(&value[1..value.len() - 1])
            } else {
                Some(value)
            };
        }
        if cursor == value.len() {
            break;
        }
        cursor += 1;
    }
    None
}

fn multipart_boundary(content_type: Option<&[u8]>) -> Option<&[u8]> {
    content_type?
        .split(|byte| *byte == b';')
        .skip(1)
        .find_map(|parameter| disposition_parameter_value(parameter, b"boundary"))
}

fn disposition_parameter_value<'input>(parameter: &'input [u8], expected: &[u8]) -> Option<&'input [u8]> {
    let (name, value) = split_once(trim_ascii(parameter), b'=');
    if !name.eq_ignore_ascii_case(expected) {
        return None;
    }
    let value = trim_ascii(value);
    if value.len() >= 2 && value.first() == Some(&b'"') && value.last() == Some(&b'"') {
        Some(&value[1..value.len() - 1])
    } else {
        Some(value)
    }
}

fn parse_xml(input: &[u8], builder: &mut BodyBuilder) -> Result<(), BodyError> {
    if input.windows(9).any(|window| window.eq_ignore_ascii_case(b"<!DOCTYPE"))
        || input.windows(8).any(|window| window.eq_ignore_ascii_case(b"<!ENTITY"))
    {
        return Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        });
    }
    let mut cursor = 0_usize;
    let mut elements: Vec<Vec<u8>> = Vec::new();
    let mut roots = 0_usize;
    while cursor < input.len() {
        let Some(open) = input[cursor..].iter().position(|byte| *byte == b'<') else {
            let text = trim_ascii(&input[cursor..]);
            if !text.is_empty() {
                if elements.is_empty() {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                }
                builder.push(CollectionKind::Xml, None, &decode_xml(text)?, false)?;
            }
            break;
        };
        let text_end = cursor.checked_add(open).ok_or(BodyError::Arithmetic)?;
        let text = trim_ascii(&input[cursor..text_end]);
        if !text.is_empty() {
            if elements.is_empty() {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            builder.push(CollectionKind::Xml, None, &decode_xml(text)?, false)?;
        }
        if input.get(text_end..text_end + 4) == Some(b"<!--") {
            let Some(end) = find_bytes(&input[text_end + 4..], b"-->") else {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            };
            cursor = text_end + 4 + end + 3;
            continue;
        }
        if input.get(text_end..text_end + 9) == Some(b"<![CDATA[") {
            if elements.is_empty() {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            let Some(end) = find_bytes(&input[text_end + 9..], b"]]>") else {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            };
            builder.push(
                CollectionKind::Xml,
                None,
                &input[text_end + 9..text_end + 9 + end],
                false,
            )?;
            cursor = text_end + 9 + end + 3;
            continue;
        }
        let Some(close) = input[text_end..].iter().position(|byte| *byte == b'>') else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        };
        let tag_end = text_end.checked_add(close).ok_or(BodyError::Arithmetic)?;
        let tag = trim_ascii(&input[text_end + 1..tag_end]);
        if tag.starts_with(b"?") {
            cursor = tag_end + 1;
            continue;
        }
        if tag.starts_with(b"/") {
            let closing = trim_ascii(&tag[1..]);
            let opening = elements.pop().ok_or(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            })?;
            if closing != opening {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
        } else {
            let self_closing = tag.ends_with(b"/");
            let opening = trim_ascii(tag.strip_suffix(b"/").unwrap_or(tag));
            let name_end = opening
                .iter()
                .position(u8::is_ascii_whitespace)
                .unwrap_or(opening.len());
            let name = &opening[..name_end];
            if name.is_empty() || name.iter().copied().any(|byte| b"<>&\"'=/".contains(&byte)) {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            if elements.is_empty() {
                roots = roots.checked_add(1).ok_or(BodyError::Arithmetic)?;
                if roots > 1 {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                }
            }
            let depth = elements.len().checked_add(1).ok_or(BodyError::Arithmetic)?;
            if depth > builder.accounting.limits.max_depth {
                return Err(BodyError::DepthLimit {
                    limit: builder.accounting.limits.max_depth,
                });
            }
            parse_xml_attributes(opening, builder)?;
            if !self_closing {
                elements.try_reserve(1).map_err(|_| BodyError::Allocation)?;
                elements.push(try_copy(name)?);
            }
        }
        builder.charge_work(tag.len())?;
        cursor = tag_end + 1;
    }
    if !elements.is_empty() || roots != 1 {
        return Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        });
    }
    Ok(())
}

fn parse_xml_attributes(tag: &[u8], builder: &mut BodyBuilder) -> Result<(), BodyError> {
    let mut cursor = tag.iter().position(u8::is_ascii_whitespace).unwrap_or(tag.len());
    while cursor < tag.len() {
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == tag.len() {
            break;
        }
        let start = cursor;
        while cursor < tag.len() && !tag[cursor].is_ascii_whitespace() && tag[cursor] != b'=' {
            cursor += 1;
        }
        let name = &tag[start..cursor];
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if tag.get(cursor) != Some(&b'=') {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        }
        cursor += 1;
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *tag.get(cursor).ok_or(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        })?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < tag.len() && tag[cursor] != quote {
            cursor += 1;
        }
        let value = tag.get(value_start..cursor).ok_or(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        })?;
        cursor = cursor.checked_add(1).ok_or(BodyError::Arithmetic)?;
        builder.push(CollectionKind::Xml, Some(name), &decode_xml(value)?, true)?;
    }
    Ok(())
}

fn decode_xml(value: &[u8]) -> Result<Vec<u8>, BodyError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| BodyError::Allocation)?;
    let mut cursor = 0_usize;
    while cursor < value.len() {
        if value[cursor] != b'&' {
            output.push(value[cursor]);
            cursor += 1;
            continue;
        }
        let Some(relative_end) = value[cursor..].iter().position(|byte| *byte == b';') else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        };
        let end = cursor + relative_end;
        let entity = &value[cursor + 1..end];
        match entity {
            b"amp" => output.push(b'&'),
            b"lt" => output.push(b'<'),
            b"gt" => output.push(b'>'),
            b"quot" => output.push(b'"'),
            b"apos" => output.push(b'\''),
            _ => {
                let (digits, radix) = if let Some(digits) = entity.strip_prefix(b"#x") {
                    (digits, 16)
                } else if let Some(digits) = entity.strip_prefix(b"#") {
                    (digits, 10)
                } else {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                };
                let digits = std::str::from_utf8(digits).map_err(|_| BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                })?;
                let scalar = u32::from_str_radix(digits, radix)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|value| is_xml_scalar(*value))
                    .ok_or(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    })?;
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes());
            }
        }
        cursor = end + 1;
    }
    Ok(output)
}

fn is_xml_scalar(value: char) -> bool {
    let value = u32::from(value);
    matches!(value, 0x09 | 0x0A | 0x0D)
        || (0x20..=0xD7FF).contains(&value)
        || (0xE000..=0xFFFD).contains(&value)
        || (0x10000..=0x10FFFF).contains(&value)
}

fn percent_decode(value: &[u8]) -> Result<Vec<u8>, BodyError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| BodyError::Allocation)?;
    let mut cursor = 0_usize;
    while cursor < value.len() {
        match value[cursor] {
            b'+' => {
                output.push(b' ');
                cursor += 1;
            }
            b'%' => {
                let decoded = value
                    .get(cursor + 1..cursor + 3)
                    .and_then(|digits| hex(digits[0]).zip(hex(digits[1])))
                    .map(|(high, low)| (high << 4) | low);
                if let Some(byte) = decoded {
                    output.push(byte);
                    cursor += 3;
                } else {
                    // ModSecurity-compatible form decoding is deliberately
                    // tolerant: malformed escapes remain visible to rules.
                    output.push(b'%');
                    cursor += 1;
                }
            }
            byte => {
                output.push(byte);
                cursor += 1;
            }
        }
    }
    Ok(output)
}

fn try_copy(value: &[u8]) -> Result<Vec<u8>, BodyError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| BodyError::Allocation)?;
    output.extend_from_slice(value);
    Ok(output)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty()).then(|| haystack.windows(needle.len()).position(|window| window == needle))?
}

fn split_once(value: &[u8], delimiter: u8) -> (&[u8], &[u8]) {
    value
        .iter()
        .position(|byte| *byte == delimiter)
        .map_or((value, &[]), |index| (&value[..index], &value[index + 1..]))
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/body_test.rs"]
mod tests;
