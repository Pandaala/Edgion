//! Request and response state: header/argument/cookie projection, collection indices and limits checks.

use super::*;

pub(super) struct RequestState<'generation> {
    pub(super) bytes: Vec<u8>,
    pub(super) collections: [Vec<CollectionValue>; COLLECTION_COUNT],
    pub(super) body: Option<RequestBodyPublication<'generation>>,
    pub(super) entry_count: usize,
    pub(super) retained_bytes: usize,
}

pub(super) struct ResponseEntry {
    pub(super) collection: CollectionKind,
    pub(super) name: Option<Vec<u8>>,
    pub(super) value: Vec<u8>,
}

pub(super) struct ResponseState {
    pub(super) entries: Vec<ResponseEntry>,
    pub(super) window: Option<Vec<u8>>,
    retained_bytes: usize,
}

impl ResponseState {
    pub(super) fn build(response: ResponseHead<'_>, limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        if !(100..=999).contains(&response.status) {
            return Err(RuntimeError::InvalidResponseStatus {
                status: response.status,
            });
        }
        if response.headers.len() > limits.max_response_headers {
            return Err(RuntimeError::ResponseHeaderLimit {
                actual: response.headers.len(),
                limit: limits.max_response_headers,
            });
        }
        let mut header_bytes = 0_usize;
        for (ordinal, header) in response.headers.iter().copied().enumerate() {
            if header.name.is_empty() || !header.name.iter().copied().all(is_header_name_byte) {
                return Err(RuntimeError::InvalidResponseHeaderName { ordinal });
            }
            if memchr::memchr3(b'\r', b'\n', 0, header.value).is_some() {
                return Err(RuntimeError::InvalidResponseHeaderValue { ordinal });
            }
            check_value(header.name, limits.max_value_bytes)?;
            check_value(header.value, limits.max_value_bytes)?;
            header_bytes = checked_add(header_bytes, checked_add(header.name.len(), header.value.len())?)?;
            if header_bytes > limits.max_response_header_bytes {
                return Err(RuntimeError::ResponseHeaderByteLimit {
                    actual: header_bytes,
                    limit: limits.max_response_header_bytes,
                });
            }
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(response.headers.len().saturating_add(1))
            .map_err(|_| RuntimeError::Allocation)?;
        let (status_digits, status_start) = decimal_bytes(usize::from(response.status));
        entries.push(ResponseEntry {
            collection: CollectionKind::ResponseStatus,
            name: None,
            value: owned_copy(&status_digits[status_start..])?,
        });
        for header in response.headers {
            entries.push(ResponseEntry {
                collection: CollectionKind::ResponseHeaders,
                name: Some(owned_copy(header.name)?),
                value: owned_copy(header.value)?,
            });
        }
        let retained_bytes = header_bytes
            .checked_add(status_digits.len() - status_start)
            .ok_or(RuntimeError::Arithmetic)?
            .checked_add(
                entries
                    .len()
                    .checked_mul(size_of::<ResponseEntry>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .ok_or(RuntimeError::Arithmetic)?;
        Ok(Self {
            entries,
            window: None,
            retained_bytes,
        })
    }

    pub(super) fn total_retained_bytes(&self) -> usize {
        self.retained_bytes
            .saturating_add(self.window.as_ref().map_or(0, Vec::len))
    }
}

pub(super) fn phase_index(phase: Phase) -> usize {
    phase as usize - 1
}

impl RequestState<'_> {
    pub(super) fn build(request: RequestHead<'_>, limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        if request.headers.len() > limits.max_headers {
            return Err(RuntimeError::HeaderLimit {
                actual: request.headers.len(),
                limit: limits.max_headers,
            });
        }
        let fixed = [
            request.request_line,
            request.method,
            request.uri,
            request.uri_raw,
            request.protocol,
            request.remote_addr,
            request.unique_id,
        ];
        let mut input_bytes = 0_usize;
        for value in fixed {
            check_value(value, limits.max_value_bytes)?;
            input_bytes = checked_add(input_bytes, value.len())?;
        }
        let mut header_bytes = 0_usize;
        for (ordinal, header) in request.headers.iter().copied().enumerate() {
            if header.name.is_empty() || !header.name.iter().copied().all(is_header_name_byte) {
                return Err(RuntimeError::InvalidHeaderName { ordinal });
            }
            if memchr::memchr3(b'\r', b'\n', 0, header.value).is_some() {
                return Err(RuntimeError::InvalidHeaderValue { ordinal });
            }
            check_value(header.name, limits.max_value_bytes)?;
            check_value(header.value, limits.max_value_bytes)?;
            header_bytes = checked_add(header_bytes, checked_add(header.name.len(), header.value.len())?)?;
            if header_bytes > limits.max_header_bytes {
                return Err(RuntimeError::HeaderByteLimit {
                    actual: header_bytes,
                    limit: limits.max_header_bytes,
                });
            }
        }
        input_bytes = checked_add(input_bytes, header_bytes)?;
        if input_bytes > limits.max_input_bytes {
            return Err(RuntimeError::InputByteLimit {
                actual: input_bytes,
                limit: limits.max_input_bytes,
            });
        }

        let query = query_part(request.uri);
        let query_count = count_delimited(query, b'&', limits.max_query_arguments)?;
        let cookie_count = count_cookies(request.headers, limits.max_cookies)?;
        let scalar_count = 12_usize;
        let header_records = checked_mul(request.headers.len(), 2)?;
        let query_records = checked_mul(query_count, 4)?;
        let cookie_records = checked_mul(cookie_count, 2)?;
        let record_count = scalar_count
            .checked_add(header_records)
            .and_then(|value| value.checked_add(query_records))
            .and_then(|value| value.checked_add(cookie_records))
            .ok_or(RuntimeError::Arithmetic)?;
        if record_count > limits.max_collection_entries {
            return Err(RuntimeError::CollectionEntryLimit {
                limit: limits.max_collection_entries,
            });
        }

        let derived_bytes = projected_derived_bytes(request, query)?;
        let retained_bytes = derived_bytes
            .nominal
            .checked_add(
                record_count
                    .checked_mul(size_of::<CollectionValue>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .and_then(|value| value.checked_add(size_of::<[Vec<CollectionValue>; COLLECTION_COUNT]>()))
            .ok_or(RuntimeError::Arithmetic)?;
        if retained_bytes > limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }

        let mut state = Self {
            bytes: Vec::new(),
            collections: std::array::from_fn(|_| Vec::new()),
            body: None,
            entry_count: 0,
            retained_bytes: 0,
        };
        state
            .bytes
            .try_reserve_exact(derived_bytes.arena)
            .map_err(|_| RuntimeError::Allocation)?;
        let (digits, digit_start) = decimal_bytes(derived_bytes.query_combined_size);
        state.push_scalar(CollectionKind::ArgsCombinedSize, &digits[digit_start..])?;
        state.push_query_collection(query)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsGet, false)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsNames, true)?;
        state.push_collection_alias(CollectionKind::Args, CollectionKind::ArgsGetNames, true)?;
        state.push_scalar(CollectionKind::QueryString, query)?;
        state.push_scalar(CollectionKind::RemoteAddr, request.remote_addr)?;
        state.push_scalar(
            CollectionKind::ReqbodyProcessor,
            derived_bytes.body_processor.as_bytes(),
        )?;
        state.push_cookie_collection(request.headers)?;
        state.push_collection_alias(
            CollectionKind::RequestCookies,
            CollectionKind::RequestCookiesNames,
            true,
        )?;
        state.push_request_path_collections(request.uri)?;
        state.push_header_collection(request.headers)?;
        state.push_collection_alias(
            CollectionKind::RequestHeaders,
            CollectionKind::RequestHeadersNames,
            true,
        )?;
        state.push_scalar(CollectionKind::RequestLine, request.request_line)?;
        state.push_scalar(CollectionKind::RequestMethod, request.method)?;
        state.push_scalar(CollectionKind::RequestProtocol, request.protocol)?;
        state.push_scalar(CollectionKind::RequestUri, request.uri)?;
        state.push_scalar(CollectionKind::RequestUriRaw, request.uri_raw)?;
        state.push_scalar(CollectionKind::UniqueId, request.unique_id)?;

        // Logical alias payload remains charged even when immutable ranges share bytes.
        state.retained_bytes = derived_bytes
            .nominal
            .checked_add(
                state
                    .entry_count
                    .checked_mul(size_of::<CollectionValue>())
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .and_then(|value| value.checked_add(size_of::<[Vec<CollectionValue>; COLLECTION_COUNT]>()))
            .ok_or(RuntimeError::Arithmetic)?;
        if state.retained_bytes > limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            });
        }
        debug_assert!(state.entry_count <= record_count);
        Ok(state)
    }

    fn push_record(&mut self, collection: CollectionKind, record: CollectionValue) -> Result<(), RuntimeError> {
        let records = &mut self.collections[collection_index(collection)];
        records.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
        records.push(record);
        self.entry_count = self.entry_count.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        Ok(())
    }

    fn push_scalar(&mut self, collection: CollectionKind, value: &[u8]) -> Result<(), RuntimeError> {
        let value = self.copy_bytes(value)?;
        self.push_record(collection, CollectionValue { name: None, value })
    }

    fn push_collection_alias(
        &mut self,
        source: CollectionKind,
        alias: CollectionKind,
        names: bool,
    ) -> Result<(), RuntimeError> {
        // Replay records in collection order, preserving fallible metadata growth order.
        let source = collection_index(source);
        for index in 0..self.collections[source].len() {
            let mut record = self.collections[source][index];
            if names {
                record.value = record.name.ok_or(RuntimeError::GenerationInvariant)?;
            }
            self.push_record(alias, record)?;
        }
        Ok(())
    }

    fn push_header_collection(&mut self, headers: &[RequestHeader<'_>]) -> Result<(), RuntimeError> {
        for header in headers {
            let name = self.copy_bytes(header.name)?;
            let value = self.copy_bytes(header.value)?;
            self.push_record(
                CollectionKind::RequestHeaders,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_query_collection(&mut self, query: &[u8]) -> Result<(), RuntimeError> {
        for segment in nonempty_segments(query, b'&') {
            let (name, value) = split_once(segment, b'=');
            let name = self.copy_percent_decoded(name)?;
            let value = self.copy_percent_decoded(value)?;
            self.push_record(
                CollectionKind::Args,
                CollectionValue {
                    name: Some(name),
                    value,
                },
            )?;
        }
        Ok(())
    }

    fn push_cookie_collection(&mut self, headers: &[RequestHeader<'_>]) -> Result<(), RuntimeError> {
        for header in headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(b"cookie"))
        {
            for segment in nonempty_segments(header.value, b';') {
                let (name, value) = split_once(trim_ascii(segment), b'=');
                let name = trim_ascii(name);
                if name.is_empty() {
                    continue;
                }
                let name = self.copy_bytes(name)?;
                let value = self.copy_bytes(trim_ascii(value))?;
                self.push_record(
                    CollectionKind::RequestCookies,
                    CollectionValue {
                        name: Some(name),
                        value,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn push_request_path_collections(&mut self, uri: &[u8]) -> Result<(), RuntimeError> {
        let filename = self.copy_percent_decoded_path(request_filename(uri))?;
        let filename_end = filename
            .start
            .checked_add(filename.len)
            .ok_or(RuntimeError::Arithmetic)?;
        let decoded = self
            .bytes
            .get(filename.start..filename_end)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let basename_offset = decoded
            .iter()
            .rposition(|byte| matches!(byte, b'/' | b'\\'))
            .map_or(0, |index| index + 1);
        let basename = ByteRange {
            start: filename
                .start
                .checked_add(basename_offset)
                .ok_or(RuntimeError::Arithmetic)?,
            len: filename.len - basename_offset,
        };
        self.push_record(
            CollectionKind::RequestBasename,
            CollectionValue {
                name: None,
                value: basename,
            },
        )?;
        self.push_record(
            CollectionKind::RequestFilename,
            CollectionValue {
                name: None,
                value: filename,
            },
        )
    }

    fn copy_bytes(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(value);
        Ok(ByteRange {
            start,
            len: value.len(),
        })
    }

    pub(super) fn copy_percent_decoded(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        if memchr::memchr2(b'+', b'%', value).is_none() {
            return self.copy_bytes(value);
        }
        let start = self.bytes.len();
        let mut index = 0_usize;
        while index < value.len() {
            match value[index] {
                b'+' => {
                    self.bytes.push(b' ');
                    index += 1;
                }
                b'%' if index + 2 < value.len() => {
                    if let (Some(high), Some(low)) = (hex(value[index + 1]), hex(value[index + 2])) {
                        self.bytes.push((high << 4) | low);
                        index += 3;
                    } else {
                        self.bytes.push(b'%');
                        index += 1;
                    }
                }
                byte => {
                    self.bytes.push(byte);
                    index += 1;
                }
            }
        }
        Ok(ByteRange {
            start,
            len: self.bytes.len() - start,
        })
    }

    pub(super) fn copy_percent_decoded_path(&mut self, value: &[u8]) -> Result<ByteRange, RuntimeError> {
        if memchr::memchr(b'%', value).is_none() {
            return self.copy_bytes(value);
        }
        let start = self.bytes.len();
        let mut index = 0_usize;
        while index < value.len() {
            if value[index] == b'%' && index + 2 < value.len() {
                if let (Some(high), Some(low)) = (hex(value[index + 1]), hex(value[index + 2])) {
                    self.bytes.push((high << 4) | low);
                    index += 3;
                    continue;
                }
            }
            self.bytes.push(value[index]);
            index += 1;
        }
        Ok(ByteRange {
            start,
            len: self.bytes.len() - start,
        })
    }

    pub(super) fn bytes_at(&self, range: ByteRange) -> &[u8] {
        range
            .start
            .checked_add(range.len)
            .and_then(|end| self.bytes.get(range.start..end))
            .unwrap_or_default()
    }

    pub(super) fn collection_records(&self, collection: CollectionKind) -> Result<&[CollectionValue], RuntimeError> {
        Ok(&self.collections[collection_index(collection)])
    }

    pub(super) fn first_named(&self, collection: CollectionKind, key: &[u8], ascii_insensitive: bool) -> Option<&[u8]> {
        let head = self.collection_records(collection).ok()?.iter().find_map(|record| {
            let name = self.bytes_at(record.name?);
            let matched = if ascii_insensitive {
                name.eq_ignore_ascii_case(key)
            } else {
                name == key
            };
            matched.then(|| self.bytes_at(record.value))
        });
        head.or_else(|| {
            self.body.as_ref()?.materialized()?.entries.iter().find_map(|entry| {
                if entry.collection != collection {
                    return None;
                }
                let name = entry.name.as_deref()?;
                let matched = if ascii_insensitive {
                    name.eq_ignore_ascii_case(key)
                } else {
                    name == key
                };
                matched.then_some(entry.value.as_slice())
            })
        })
    }

    pub(super) fn first_value(&self, collection: CollectionKind) -> Option<&[u8]> {
        if let Some(value) = self
            .body
            .as_ref()
            .and_then(RequestBodyPublication::json)
            .and_then(|body| body.metadata_value(collection))
        {
            return Some(value);
        }
        if collection == CollectionKind::ArgsCombinedSize {
            if let Some(body) = self.body.as_ref().and_then(RequestBodyPublication::materialized) {
                if let Some(value) = body
                    .entries
                    .iter()
                    .find_map(|entry| (entry.collection == collection).then_some(entry.value.as_slice()))
                {
                    return Some(value);
                }
            }
        }
        self.collection_records(collection)
            .ok()?
            .first()
            .map(|record| self.bytes_at(record.value))
            .or_else(|| {
                self.body
                    .as_ref()?
                    .materialized()?
                    .entries
                    .iter()
                    .find_map(|entry| (entry.collection == collection).then_some(entry.value.as_slice()))
            })
    }
}

pub(super) fn phase_from_number(value: u8) -> Option<Phase> {
    match value {
        1 => Some(Phase::RequestHeaders),
        2 => Some(Phase::RequestBody),
        3 => Some(Phase::ResponseHeaders),
        4 => Some(Phase::ResponseBody),
        5 => Some(Phase::Logging),
        _ => None,
    }
}

pub(super) fn collection_index(collection: CollectionKind) -> usize {
    match collection {
        CollectionKind::Args => 0,
        CollectionKind::ArgsCombinedSize => 1,
        CollectionKind::ArgsGet => 2,
        CollectionKind::ArgsGetNames => 3,
        CollectionKind::ArgsNames => 4,
        CollectionKind::Files => 5,
        CollectionKind::FilesCombinedSize => 6,
        CollectionKind::FilesNames => 7,
        CollectionKind::MatchedVar => 8,
        CollectionKind::MatchedVars => 9,
        CollectionKind::MultipartPartHeaders => 10,
        CollectionKind::QueryString => 11,
        CollectionKind::RemoteAddr => 12,
        CollectionKind::ReqbodyProcessor => 13,
        CollectionKind::RequestBasename => 14,
        CollectionKind::RequestBody => 15,
        CollectionKind::RequestBodyLength => 16,
        CollectionKind::RequestCookies => 17,
        CollectionKind::RequestCookiesNames => 18,
        CollectionKind::RequestFilename => 19,
        CollectionKind::RequestHeaders => 20,
        CollectionKind::RequestHeadersNames => 21,
        CollectionKind::RequestLine => 22,
        CollectionKind::RequestMethod => 23,
        CollectionKind::RequestProtocol => 24,
        CollectionKind::RequestUri => 25,
        CollectionKind::RequestUriRaw => 26,
        CollectionKind::ResponseBody => 27,
        CollectionKind::ResponseHeaders => 28,
        CollectionKind::ResponseStatus => 29,
        CollectionKind::Tx => 30,
        CollectionKind::UniqueId => 31,
        CollectionKind::Xml => 32,
    }
}

fn check_value(value: &[u8], limit: usize) -> Result<(), RuntimeError> {
    if value.len() > limit {
        return Err(RuntimeError::ValueByteLimit {
            actual: value.len(),
            limit,
        });
    }
    Ok(())
}

fn checked_add(left: usize, right: usize) -> Result<usize, RuntimeError> {
    left.checked_add(right).ok_or(RuntimeError::Arithmetic)
}

fn checked_mul(left: usize, right: usize) -> Result<usize, RuntimeError> {
    left.checked_mul(right).ok_or(RuntimeError::Arithmetic)
}

fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

pub(super) fn query_part(uri: &[u8]) -> &[u8] {
    memchr::memchr(b'?', uri).map_or(&[], |index| &uri[index + 1..])
}

pub(super) fn request_filename(uri: &[u8]) -> &[u8] {
    let end = memchr::memchr(b'?', uri).unwrap_or(uri.len());
    &uri[..end]
}

fn nonempty_segments(value: &[u8], delimiter: u8) -> impl Iterator<Item = &[u8]> {
    value
        .split(move |byte| *byte == delimiter)
        .filter(|segment| !segment.is_empty())
}

fn count_delimited(value: &[u8], delimiter: u8, limit: usize) -> Result<usize, RuntimeError> {
    let mut count = 0_usize;
    for _ in nonempty_segments(value, delimiter) {
        if count == limit {
            return Err(RuntimeError::QueryArgumentLimit { limit });
        }
        count += 1;
    }
    Ok(count)
}

fn count_cookies(headers: &[RequestHeader<'_>], limit: usize) -> Result<usize, RuntimeError> {
    let mut count = 0_usize;
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(b"cookie"))
    {
        for segment in nonempty_segments(header.value, b';') {
            let (name, _) = split_once(trim_ascii(segment), b'=');
            if trim_ascii(name).is_empty() {
                continue;
            }
            if count == limit {
                return Err(RuntimeError::CookieLimit { limit });
            }
            count += 1;
        }
    }
    Ok(count)
}

pub(super) fn split_once(value: &[u8], delimiter: u8) -> (&[u8], &[u8]) {
    memchr::memchr(delimiter, value).map_or((value, &[]), |index| (&value[..index], &value[index + 1..]))
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

pub(super) fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn query_argument_combined_size(query: &[u8]) -> Result<usize, RuntimeError> {
    nonempty_segments(query, b'&').try_fold(0_usize, |total, segment| {
        let (name, value) = split_once(segment, b'=');
        checked_add(
            total,
            checked_add(percent_decoded_len(name), percent_decoded_len(value))?,
        )
    })
}

pub(super) fn percent_decoded_len(value: &[u8]) -> usize {
    if memchr::memchr(b'%', value).is_none() {
        return value.len();
    }
    let mut length = 0_usize;
    let mut index = 0_usize;
    while index < value.len() {
        index += if value[index] == b'%'
            && index + 2 < value.len()
            && hex(value[index + 1]).is_some()
            && hex(value[index + 2]).is_some()
        {
            3
        } else {
            1
        };
        length += 1;
    }
    length
}

pub(super) struct RequestHeadProjection {
    pub(super) arena: usize,
    nominal: usize,
    pub(super) query_combined_size: usize,
    body_processor: BodyProcessorSelection,
}

pub(super) fn projected_derived_bytes(
    request: RequestHead<'_>,
    query: &[u8],
) -> Result<RequestHeadProjection, RuntimeError> {
    let query_combined_size = query_argument_combined_size(query)?;
    let mut total = query_combined_size
        .checked_ilog10()
        .map_or(1, |digits| digits as usize + 1);
    let body_processor = request_body_processor(request.headers);
    for value in [
        query,
        request.remote_addr,
        body_processor.as_bytes(),
        request.request_line,
        request.method,
        request.protocol,
        request.uri,
        request.uri_raw,
        request.unique_id,
    ] {
        total = checked_add(total, value.len())?;
    }
    total = checked_add(total, percent_decoded_len(request_filename(request.uri)))?;
    let mut arena = total;
    for segment in nonempty_segments(query, b'&') {
        let (name, value) = split_once(segment, b'=');
        let name = percent_decoded_len(name);
        let value = percent_decoded_len(value);
        total = checked_add(total, checked_mul(name, 4)?)?;
        arena = checked_add(arena, name)?;
        total = checked_add(total, checked_mul(value, 2)?)?;
        arena = checked_add(arena, value)?;
    }
    for header in request.headers {
        total = checked_add(total, checked_mul(header.name.len(), 2)?)?;
        arena = checked_add(arena, header.name.len())?;
        total = checked_add(total, header.value.len())?;
        arena = checked_add(arena, header.value.len())?;
        if header.name.eq_ignore_ascii_case(b"cookie") {
            for segment in nonempty_segments(header.value, b';') {
                let (name, value) = split_once(trim_ascii(segment), b'=');
                let name = trim_ascii(name);
                if name.is_empty() {
                    continue;
                }
                total = checked_add(total, checked_mul(name.len(), 2)?)?;
                arena = checked_add(arena, name.len())?;
                let value_len = trim_ascii(value).len();
                total = checked_add(total, value_len)?;
                arena = checked_add(arena, value_len)?;
            }
        }
    }
    Ok(RequestHeadProjection {
        arena,
        nominal: total,
        query_combined_size,
        body_processor,
    })
}

fn request_body_processor(headers: &[RequestHeader<'_>]) -> BodyProcessorSelection {
    processor_from_content_type(
        headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(b"content-type"))
            .map(|header| header.value),
    )
}
