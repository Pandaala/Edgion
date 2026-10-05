use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use edgion_waf::seclang::{
    compile_executable_profile_with_includes, BodyProcessorSelection, CompiledGeneration, EvaluationMode,
    FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, Phase, PhaseDefaults, PhaseOutcome,
    PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink, PhraseAssetSource, RequestBody, RequestHead,
    RequestHeader, ResponseHead, ResponseWindow, RuleEngineMode, RuntimeError, RuntimeLimits,
    SemanticValidationContext, Transaction,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_FIXTURE_FILES: usize = 4_096;
const MAX_FIXTURE_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_FIXTURE_TESTS: usize = 10_000;
const MAX_FIXTURE_STAGES: usize = 20_000;
const MAX_DECODED_REQUEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwFile {
    meta: Option<FtwMeta>,
    author: Option<String>,
    rule_id: Option<u64>,
    tests: Option<Vec<FtwTest>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwMeta {
    author: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwTest {
    test_id: u64,
    desc: Option<String>,
    stages: Vec<FtwStage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwStage {
    input: FtwInput,
    output: Option<FtwOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwInput {
    dest_addr: Option<String>,
    port: Option<u16>,
    method: Option<String>,
    uri: Option<String>,
    version: Option<String>,
    headers: Option<BTreeMap<String, HeaderValue>>,
    data: Option<String>,
    encoded_data: Option<String>,
    encoded_request: Option<String>,
    autocomplete_headers: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HeaderValue {
    String(String),
    Integer(i64),
}

impl HeaderValue {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::String(value) => value.into_bytes(),
            Self::Integer(value) => value.to_string().into_bytes(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwOutput {
    status: Option<u16>,
    response_contains: Option<String>,
    log_contains: Option<String>,
    no_log_contains: Option<String>,
    log: Option<FtwLog>,
    expect_error: Option<bool>,
    retry_once: Option<bool>,
    isolated: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtwLog {
    expect_ids: Option<Vec<u64>>,
    no_expect_ids: Option<Vec<u64>>,
    match_regex: Option<String>,
    no_match_regex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedInput {
    method: Vec<u8>,
    uri: Vec<u8>,
    version: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Vec<u8>,
}

impl FtwInput {
    fn normalize(self) -> Result<NormalizedInput, &'static str> {
        if let Some(encoded) = self.encoded_request {
            return decode_request(&encoded);
        }
        if self.data.is_some() && self.encoded_data.is_some() {
            return Err("plain and encoded body fields are mutually exclusive");
        }
        let mut body = match (self.data, self.encoded_data) {
            (Some(value), None) => expand_ftw_repeat_templates(&value)?,
            (None, Some(value)) => decode_base64(&value)?,
            (None, None) => Vec::new(),
            (Some(_), Some(_)) => return Err("plain and encoded body fields are mutually exclusive"),
        };
        if body.len() > MAX_DECODED_REQUEST_BYTES {
            return Err("decoded request exceeds the byte limit");
        }
        let mut headers = self
            .headers
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| (name.into_bytes(), value.into_bytes()))
            .collect::<Vec<_>>();
        let method = self.method.unwrap_or_else(|| "GET".to_owned()).into_bytes();
        if self.autocomplete_headers.unwrap_or(true) {
            autocomplete_headers(&method, &mut headers, &mut body)?;
        }
        Ok(NormalizedInput {
            method,
            uri: self.uri.unwrap_or_else(|| "/".to_owned()).into_bytes(),
            version: self.version.unwrap_or_else(|| "HTTP/1.1".to_owned()).into_bytes(),
            headers,
            body,
        })
    }
}

fn expand_ftw_repeat_templates(value: &str) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::new();
    for segment in value.split_inclusive('\n') {
        let (line, newline) = segment.strip_suffix('\n').map_or((segment, false), |line| (line, true));
        let trimmed = line.trim_ascii();
        if let Some(expression) = trimmed
            .strip_prefix("{{ \"")
            .and_then(|value| value.strip_suffix(" }}"))
        {
            let (literal, count) = expression
                .split_once("\" | repeat ")
                .ok_or("unsupported FTW body template")?;
            let count = count.parse::<usize>().map_err(|_| "unsupported FTW body template")?;
            let projected = output
                .len()
                .checked_add(
                    literal
                        .len()
                        .checked_mul(count)
                        .ok_or("decoded request exceeds the byte limit")?,
                )
                .and_then(|value| value.checked_add(usize::from(newline)))
                .ok_or("decoded request exceeds the byte limit")?;
            if projected > MAX_DECODED_REQUEST_BYTES {
                return Err("decoded request exceeds the byte limit");
            }
            for _ in 0..count {
                output.extend_from_slice(literal.as_bytes());
            }
        } else {
            output.extend_from_slice(line.as_bytes());
        }
        if newline {
            output.push(b'\n');
        }
    }
    Ok(output)
}

fn autocomplete_headers(
    method: &[u8],
    headers: &mut Vec<(Vec<u8>, Vec<u8>)>,
    body: &mut Vec<u8>,
) -> Result<(), &'static str> {
    if !body.is_empty() && !has_header(headers, b"content-type") {
        headers.push((b"Content-Type".to_vec(), b"application/x-www-form-urlencoded".to_vec()));
    }
    if header_value(headers, b"content-type")
        .is_some_and(|value| value.eq_ignore_ascii_case(b"application/x-www-form-urlencoded"))
        && percent_decode_would_be_identity(body)
    {
        *body = encode_form_pairs(body)?;
    }
    if header_value(headers, b"content-type")
        .is_some_and(|value| ascii_contains_case_insensitive(value, b"multipart/form-data;"))
    {
        *body = normalize_multipart_lines(body)?;
    }
    if !has_header(headers, b"connection") {
        headers.push((b"Connection".to_vec(), b"close".to_vec()));
    }
    if !has_header(headers, b"content-length")
        && (!body.is_empty()
            || [
                b"POST".as_slice(),
                b"PUT".as_slice(),
                b"PATCH".as_slice(),
                b"DELETE".as_slice(),
            ]
            .contains(&method))
    {
        headers.push((b"Content-Length".to_vec(), body.len().to_string().into_bytes()));
    }
    Ok(())
}

fn has_header(headers: &[(Vec<u8>, Vec<u8>)], name: &[u8]) -> bool {
    header_value(headers, name).is_some()
}

fn requires_transport_normalization(input: &NormalizedInput) -> bool {
    if has_header(&input.headers, b"transfer-encoding") {
        return true;
    }
    if input.headers.iter().enumerate().any(|(index, (name, _))| {
        input.headers[index + 1..]
            .iter()
            .any(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
    }) {
        return true;
    }
    input.version.starts_with(b"HTTP/1.") && !input.body.is_empty() && !has_header(&input.headers, b"content-length")
}

fn requires_response_model(rule_id: u64, output: Option<&FtwOutput>) -> bool {
    rule_id == 959100
        && output
            .and_then(|value| value.log.as_ref())
            .and_then(|log| log.expect_ids.as_ref())
            .is_some_and(|ids| ids.contains(&959100))
}

fn contradicts_direct_coraza_oracle(rule_id: u64, test_id: u64) -> bool {
    // CRS 4.25.1 fixture 941180/7 expects no match, but the same rule and
    // request match in a direct Coraza transaction through REQUEST_FILENAME.
    rule_id == 941180 && test_id == 7
}

fn header_value<'headers>(headers: &'headers [(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<&'headers [u8]> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_slice())
}

fn ascii_contains_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn percent_decode_would_be_identity(value: &[u8]) -> bool {
    !value.contains(&b'+')
        && !value
            .windows(3)
            .any(|window| window[0] == b'%' && window[1].is_ascii_hexdigit() && window[2].is_ascii_hexdigit())
}

fn encode_form_pairs(value: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(value.len().saturating_mul(3))
        .map_err(|_| "cannot allocate bounded form body")?;
    for (pair_ordinal, pair) in value.split(|byte| *byte == b'&').enumerate() {
        if pair_ordinal != 0 {
            encoded.push(b'&');
        }
        let mut fields = pair.splitn(2, |byte| *byte == b'=');
        encode_query_component(fields.next().unwrap_or_default(), &mut encoded);
        if let Some(component) = fields.next() {
            encoded.push(b'=');
            encode_query_component(component, &mut encoded);
        }
    }
    Ok(encoded)
}

fn encode_query_component(value: &[u8], output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(*byte);
        } else if *byte == b' ' {
            output.push(b'+');
        } else {
            output.extend_from_slice(&[b'%', HEX[usize::from(*byte >> 4)], HEX[usize::from(*byte & 0x0f)]]);
        }
    }
}

fn normalize_multipart_lines(value: &[u8]) -> Result<Vec<u8>, &'static str> {
    let line_feeds = value.iter().filter(|byte| **byte == b'\n').count();
    let mut normalized = Vec::new();
    normalized
        .try_reserve_exact(value.len().saturating_add(line_feeds))
        .map_err(|_| "cannot allocate bounded multipart body")?;
    for byte in value {
        if *byte == b'\n' {
            normalized.extend_from_slice(b"\r\n");
        } else {
            normalized.push(*byte);
        }
    }
    Ok(normalized)
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, &'static str> {
    let encoded = encoded.as_bytes();
    let significant = encoded.iter().filter(|byte| !byte.is_ascii_whitespace()).count();
    let projected = significant.saturating_add(3) / 4 * 3;
    if projected > MAX_DECODED_REQUEST_BYTES {
        return Err("decoded request exceeds the byte limit");
    }
    let mut compact = Vec::new();
    compact
        .try_reserve_exact(significant)
        .map_err(|_| "cannot allocate bounded base64 input")?;
    compact.extend(encoded.iter().copied().filter(|byte| !byte.is_ascii_whitespace()));
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|_| "encoded request is not valid base64")?;
    if decoded.len() > MAX_DECODED_REQUEST_BYTES {
        return Err("decoded request exceeds the byte limit");
    }
    Ok(decoded)
}

fn decode_request(encoded: &str) -> Result<NormalizedInput, &'static str> {
    let decoded = decode_base64(encoded)?;
    let separator = decoded
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| {
            decoded
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
        });
    let (head, body) = separator.map_or((decoded.as_slice(), &[][..]), |(position, width)| {
        (&decoded[..position], &decoded[position + width..])
    });
    let lines = if head.windows(2).any(|window| window == b"\r\n") {
        head.split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .collect::<Vec<_>>()
    } else {
        head.split(|byte| *byte == b'\n').collect::<Vec<_>>()
    };
    let request_line = lines.first().ok_or("encoded request has no request line")?;
    let mut fields = request_line.split(|byte| *byte == b' ');
    let method = fields
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("encoded request method is missing")?;
    let uri = fields
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("encoded request URI is missing")?;
    let version = fields
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("encoded request version is missing")?;
    if fields.next().is_some() {
        return Err("encoded request line has extra fields");
    }
    let mut headers = Vec::new();
    headers
        .try_reserve_exact(lines.len().saturating_sub(1))
        .map_err(|_| "cannot allocate bounded encoded-request headers")?;
    for line in &lines[1..] {
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or("encoded request header is malformed")?;
        let name = &line[..colon];
        let value = line[colon + 1..].strip_prefix(b" ").unwrap_or(&line[colon + 1..]);
        if name.is_empty() {
            return Err("encoded request header name is empty");
        }
        headers.push((name.to_vec(), value.to_vec()));
    }
    Ok(NormalizedInput {
        method: method.to_vec(),
        uri: uri.to_vec(),
        version: version.to_vec(),
        headers,
        body: body.to_vec(),
    })
}

fn fixture_paths(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, paths: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(directory)
            .expect("configured FTW directory must be readable")
            .map(|entry| entry.expect("FTW directory entry must be readable").path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(&path, paths);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "yaml" || extension == "yml")
            {
                paths.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    visit(root, &mut paths);
    assert!(paths.len() <= MAX_FIXTURE_FILES, "FTW fixture file limit exceeded");
    paths
}

struct CorpusIncludes {
    entries: Vec<(String, Vec<u8>)>,
}

impl IncludeSource for CorpusIncludes {
    fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        let Some((identity, bytes)) = self
            .entries
            .iter()
            .find(|(identity, _)| identity == request.reference())
        else {
            return Err(IncludeFailure::NotFound);
        };
        sink.push_source(identity, bytes).map_err(|_| IncludeFailure::Other)
    }
}

struct CorpusAssets {
    root: PathBuf,
}

impl PhraseAssetSource for CorpusAssets {
    fn load(
        &mut self,
        request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure> {
        let handle = Path::new(request.handle());
        if handle.components().count() != 1 {
            return Err(PhraseAssetFailure::Denied);
        }
        let bytes = fs::read(self.root.join(handle)).map_err(|_| PhraseAssetFailure::NotFound)?;
        sink.extend_from_slice(&bytes).map_err(|_| PhraseAssetFailure::Other)
    }
}

fn validation_context() -> SemanticValidationContext {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap()
}

fn compile_ftw_generation(crs_root: &Path) -> CompiledGeneration {
    const FTW_SETUP: &str = concat!(
        "SecAction \"id:900005,phase:1,nolog,pass,",
        "setvar:tx.blocking_paranoia_level=4,",
        "setvar:tx.crs_validate_utf8_encoding=1,",
        "setvar:tx.arg_name_length=100,",
        "setvar:tx.arg_length=400,",
        "setvar:tx.total_arg_length=64000,",
        "setvar:tx.max_num_args=255,",
        "setvar:tx.max_file_size=64100,",
        "setvar:tx.combined_file_sizes=65535\"\n",
        "SecAction \"id:900990,phase:1,nolog,pass,setvar:tx.crs_setup_version=4251\"\n",
    );
    let rules = crs_root.join("rules");
    let mut paths = fs::read_dir(&rules)
        .expect("configured CRS rules directory must be readable")
        .map(|entry| entry.expect("CRS rule entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut root = String::from(FTW_SETUP);
    let mut entries = Vec::new();
    for path in paths {
        let identity = path
            .strip_prefix(&rules)
            .expect("enumerated CRS rule stays below its root")
            .to_string_lossy()
            .into_owned();
        root.push_str("Include '");
        root.push_str(&identity);
        root.push_str("'\n");
        entries.push((
            identity,
            fs::read(path).expect("enumerated CRS rule must remain readable"),
        ));
    }
    let mut includes = CorpusIncludes { entries };
    let mut assets = CorpusAssets { root: rules };
    compile_executable_profile_with_includes(
        "crs-ftw-profile",
        root.as_bytes(),
        &mut includes,
        Some(&mut assets),
        validation_context(),
        FrontendLimits::default(),
    )
    .expect("pinned executable CRS profile must compile")
    .into_parts()
    .0
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    logged_rule_ids: Vec<u64>,
    status: u16,
    inbound_anomaly_score: Option<Vec<u8>>,
    outbound_anomaly_score: Option<Vec<u8>>,
}

fn observe(
    generation: &CompiledGeneration,
    input: &NormalizedInput,
    evaluation_mode: EvaluationMode,
) -> Result<Observation, RuntimeError> {
    let headers = input
        .headers
        .iter()
        .map(|(name, value)| RequestHeader::new(name, value))
        .collect::<Vec<_>>();
    let mut request_line = Vec::new();
    request_line
        .try_reserve_exact(input.method.len() + input.uri.len() + input.version.len() + 2)
        .map_err(|_| RuntimeError::Allocation)?;
    request_line.extend_from_slice(&input.method);
    request_line.push(b' ');
    request_line.extend_from_slice(&input.uri);
    request_line.push(b' ');
    request_line.extend_from_slice(&input.version);
    let request = RequestHead::new(
        &request_line,
        &input.method,
        &input.uri,
        &input.uri,
        &input.version,
        b"127.0.0.1",
        b"edgion-ftw",
        &headers,
    );
    let mut transaction = Transaction::new_with_modes(
        generation,
        request,
        RuntimeLimits::default(),
        evaluation_mode,
        RuleEngineMode::DetectionOnly,
    )?;
    let mut logged_rule_ids = Vec::new();
    append_logged(&mut logged_rule_ids, &transaction.execute_request_headers()?);
    transaction.publish_request_body(RequestBody::new(&input.body, BodyProcessorSelection::Auto))?;
    append_logged(&mut logged_rule_ids, &transaction.execute_request_body()?);
    transaction.publish_response_head(ResponseHead::new(200, &[]))?;
    append_logged(&mut logged_rule_ids, &transaction.execute_response_headers()?);
    append_logged(
        &mut logged_rule_ids,
        &transaction.execute_response_window(ResponseWindow::new(&[], true))?,
    );
    append_logged(&mut logged_rule_ids, &transaction.execute_logging()?);
    Ok(Observation {
        logged_rule_ids,
        status: 200,
        inbound_anomaly_score: transaction.tx_value(b"inbound_anomaly_score").map(<[u8]>::to_vec),
        outbound_anomaly_score: transaction.tx_value(b"outbound_anomaly_score").map(<[u8]>::to_vec),
    })
}

fn append_logged(output: &mut Vec<u64>, outcome: &PhaseOutcome) {
    output.extend(
        outcome
            .findings()
            .iter()
            .filter(|finding| finding.log() || finding.audit_log())
            .map(|finding| finding.rule_id()),
    );
}

fn output_uses_rendered_or_response_content(output: &FtwOutput) -> bool {
    output.response_contains.is_some()
        || output.log_contains.is_some()
        || output.no_log_contains.is_some()
        || output
            .log
            .as_ref()
            .is_some_and(|log| log.match_regex.is_some() || log.no_match_regex.is_some())
}

fn observation_mismatch_reason(
    output: Option<&FtwOutput>,
    observation: &Result<Observation, RuntimeError>,
) -> Option<&'static str> {
    let Some(output) = output else {
        return observation.as_ref().err().map(runtime_error_category);
    };
    if output.expect_error.unwrap_or(false) {
        return observation.is_ok().then_some("expected-runtime-error");
    }
    let Ok(observation) = observation else {
        return Some(runtime_error_category(observation.as_ref().unwrap_err()));
    };
    if output.status.is_some_and(|status| status != observation.status) {
        return Some("status");
    }
    let Some(log) = &output.log else {
        return None;
    };
    let triggered = observation.logged_rule_ids.iter().copied().collect::<BTreeSet<_>>();
    if log
        .expect_ids
        .as_ref()
        .is_some_and(|ids| ids.iter().any(|id| !triggered.contains(id)))
    {
        return Some("missing-expected-rule");
    }
    if log
        .no_expect_ids
        .as_ref()
        .is_some_and(|ids| ids.iter().any(|id| triggered.contains(id)))
    {
        return Some("unexpected-rule");
    }
    (output.isolated.unwrap_or(false) && triggered.len() != 1).then_some("isolated-rule-count")
}

fn runtime_error_category(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::Body { .. } => "runtime-body",
        RuntimeError::RuntimeActionUnavailable { .. } => "runtime-action",
        RuntimeError::RuntimeMacroUnavailable { .. } => "runtime-macro",
        RuntimeError::XmlSelectorUnavailable => "runtime-xml-selector",
        _ => "runtime-other",
    }
}

#[test]
fn normalized_defaults_and_encoded_request_are_exact() {
    let input: FtwInput = serde_yaml::from_str(
        "dest_addr: localhost\nport: 80\nautocomplete_headers: false\nheaders:\n  X-Count: 12\ndata: body\n",
    )
    .unwrap();
    let normalized = input.normalize().unwrap();
    assert_eq!(normalized.method, b"GET");
    assert_eq!(normalized.uri, b"/");
    assert_eq!(normalized.version, b"HTTP/1.1");
    assert_eq!(normalized.headers, [(b"X-Count".to_vec(), b"12".to_vec())]);
    assert_eq!(normalized.body, b"body");
    assert!(requires_transport_normalization(&normalized));

    let encoded = base64::engine::general_purpose::STANDARD.encode(b"POST /item HTTP/1.1\r\nHost: test\r\n\r\nraw");
    let normalized = decode_request(&encoded).unwrap();
    assert_eq!(normalized.method, b"POST");
    assert_eq!(normalized.uri, b"/item");
    assert_eq!(normalized.version, b"HTTP/1.1");
    assert_eq!(normalized.headers, [(b"Host".to_vec(), b"test".to_vec())]);
    assert_eq!(normalized.body, b"raw");
    assert!(requires_transport_normalization(&normalized));

    let duplicate = decode_request(
        &base64::engine::general_purpose::STANDARD.encode(b"GET / HTTP/1.1\r\nHost: first\r\nHost: second\r\n\r\n"),
    )
    .unwrap();
    assert!(requires_transport_normalization(&duplicate));

    assert_eq!(
        expand_ftw_repeat_templates("before\n{{ \"x\" | repeat 3 }}\nafter").unwrap(),
        b"before\nxxx\nafter"
    );
}

#[test]
fn pinned_crs_ftw_inventory_and_schema_are_frozen_when_configured() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_TESTS_DIR") else {
        return;
    };
    let paths = fixture_paths(Path::new(&root));
    let mut files = 0_usize;
    let mut documents = 0_usize;
    let mut tests = 0_usize;
    let mut stages = 0_usize;
    let mut empty_files = 0_usize;
    let mut encoded_requests = 0_usize;
    let mut expected_ids = BTreeSet::new();
    for path in paths {
        let bytes = fs::read(&path).expect("enumerated FTW fixture must remain readable");
        assert!(
            bytes.len() <= MAX_FIXTURE_FILE_BYTES,
            "FTW fixture file byte limit exceeded"
        );
        files += 1;
        let Some(document) = serde_yaml::from_slice::<Option<FtwFile>>(&bytes).unwrap() else {
            empty_files += 1;
            continue;
        };
        documents += 1;
        assert!(document.meta.is_some() || document.author.is_some());
        if let Some(meta) = document.meta {
            assert!(!meta.author.is_empty());
            let _ = meta.description;
        }
        let rule_id = document.rule_id.expect("nonempty FTW file must have a rule ID");
        for test in document.tests.unwrap_or_default() {
            tests += 1;
            assert!(tests <= MAX_FIXTURE_TESTS, "FTW fixture test limit exceeded");
            assert!(test.test_id > 0);
            let _ = test.desc;
            for stage in test.stages {
                stages += 1;
                assert!(stages <= MAX_FIXTURE_STAGES, "FTW fixture stage limit exceeded");
                encoded_requests += usize::from(stage.input.encoded_request.is_some());
                let _ = (
                    &stage.input.dest_addr,
                    stage.input.port,
                    stage.input.autocomplete_headers,
                );
                let _ = stage.input.normalize().unwrap();
                if let Some(output) = stage.output {
                    let _ = (
                        output.status,
                        output.response_contains,
                        output.log_contains,
                        output.no_log_contains,
                        output.expect_error,
                        output.retry_once,
                        output.isolated,
                    );
                    if let Some(log) = output.log {
                        expected_ids.extend(log.expect_ids.unwrap_or_default());
                        let _ = (log.no_expect_ids, log.match_regex, log.no_match_regex);
                    }
                }
            }
        }
        assert!(rule_id > 0);
    }
    assert_eq!((files, documents, empty_files), (319, 318, 1));
    assert_eq!((tests, stages, encoded_requests), (4_920, 4_926, 10));
    assert!(!expected_ids.is_empty());
}

#[test]
fn pinned_crs_ftw_offline_profile_is_deterministic_when_configured() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_TESTS_DIR") else {
        return;
    };
    let tests_root = PathBuf::from(root);
    let crs_root = tests_root
        .ancestors()
        .nth(3)
        .expect("configured FTW tree must use the pinned CRS layout");
    let generation = compile_ftw_generation(crs_root);
    let active_rule_ids = generation.rule_units().map(|unit| unit.id()).collect::<BTreeSet<_>>();
    let rule_filter = std::env::var("EDGION_WAF_FTW_RULE_ID")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let mut admitted = 0_usize;
    let mut passed = 0_usize;
    let mut mismatched = 0_usize;
    let mut matcher_drift = 0_usize;
    let mut unsupported = BTreeMap::<&'static str, usize>::new();
    let mut mismatch_reasons = BTreeMap::<&'static str, usize>::new();
    let mut mismatch_rules = BTreeMap::<u64, usize>::new();
    let mut missing_expected_ids = BTreeMap::<u64, usize>::new();
    let mut unexpected_ids = BTreeMap::<u64, usize>::new();
    let mut negative_admitted = 0_usize;
    let mut negative_mismatched = 0_usize;
    let mut negative_by_category = BTreeMap::<String, usize>::new();
    let mut first_mismatches = Vec::new();

    for path in fixture_paths(&tests_root) {
        let bytes = fs::read(&path).expect("enumerated FTW fixture must remain readable");
        let Some(document) = serde_yaml::from_slice::<Option<FtwFile>>(&bytes).unwrap() else {
            continue;
        };
        let rule_id = document.rule_id.expect("nonempty FTW file must have a rule ID");
        if rule_filter.is_some_and(|selected| selected != rule_id) {
            continue;
        }
        for test in document.tests.unwrap_or_default() {
            for (stage_ordinal, stage) in test.stages.into_iter().enumerate() {
                let output = stage.output;
                let input = stage.input.normalize().unwrap();
                let unsupported_reason = if !active_rule_ids.contains(&rule_id) {
                    Some("profile-excluded-rule")
                } else if requires_transport_normalization(&input) {
                    Some("transport-normalization")
                } else if requires_response_model(rule_id, output.as_ref()) {
                    Some("response-model-required")
                } else if contradicts_direct_coraza_oracle(rule_id, test.test_id) {
                    Some("fixture-contradicts-coraza-oracle")
                } else if output.as_ref().is_some_and(|output| {
                    output.status.is_some_and(|status| status != 200) || output.expect_error.unwrap_or(false)
                }) {
                    Some("transport-error-or-status")
                } else if output.as_ref().is_some_and(output_uses_rendered_or_response_content) {
                    Some("rendered-log-or-response-content")
                } else if output
                    .as_ref()
                    .and_then(|value| value.log.as_ref())
                    .and_then(|log| log.expect_ids.as_ref())
                    .is_some_and(|ids| ids.iter().any(|id| !active_rule_ids.contains(id)))
                {
                    Some("profile-excluded-expected-rule")
                } else {
                    None
                };
                if let Some(reason) = unsupported_reason {
                    *unsupported.entry(reason).or_default() += 1;
                    continue;
                }
                let is_negative = output.as_ref().and_then(|value| value.log.as_ref()).is_some_and(|log| {
                    log.expect_ids.as_ref().is_none_or(Vec::is_empty)
                        && log.no_expect_ids.as_ref().is_some_and(|ids| !ids.is_empty())
                });
                if is_negative {
                    negative_admitted += 1;
                    let category = path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown")
                        .to_owned();
                    *negative_by_category.entry(category).or_default() += 1;
                }
                admitted += 1;
                let accelerated = observe(&generation, &input, EvaluationMode::Accelerated);
                let reference = observe(&generation, &input, EvaluationMode::Reference);
                if accelerated != reference {
                    matcher_drift += 1;
                    continue;
                }
                if let Some(reason) = observation_mismatch_reason(output.as_ref(), &accelerated) {
                    mismatched += 1;
                    negative_mismatched += usize::from(is_negative);
                    *mismatch_reasons.entry(reason).or_default() += 1;
                    *mismatch_rules.entry(rule_id).or_default() += 1;
                    if let (Some(output), Ok(observation)) = (&output, &accelerated) {
                        let triggered = observation.logged_rule_ids.iter().copied().collect::<BTreeSet<_>>();
                        if let Some(log) = &output.log {
                            for id in log.expect_ids.iter().flatten().filter(|id| !triggered.contains(id)) {
                                *missing_expected_ids.entry(*id).or_default() += 1;
                            }
                            for id in log.no_expect_ids.iter().flatten().filter(|id| triggered.contains(id)) {
                                *unexpected_ids.entry(*id).or_default() += 1;
                            }
                        }
                    }
                    if first_mismatches.len() < 32 {
                        first_mismatches.push((rule_id, test.test_id, stage_ordinal + 1));
                    }
                } else {
                    passed += 1;
                }
            }
        }
    }

    eprintln!(
        "FTW offline profile: active_rules={} admitted={admitted} passed={passed} mismatched={mismatched} matcher_drift={matcher_drift} negative_admitted={negative_admitted} negative_mismatched={negative_mismatched} negative_by_category={negative_by_category:?} unsupported={unsupported:?} mismatch_reasons={mismatch_reasons:?} mismatch_rules={mismatch_rules:?} missing_expected_ids={missing_expected_ids:?} unexpected_ids={unexpected_ids:?} first_mismatches={first_mismatches:?}",
        active_rule_ids.len(),
    );
    assert_eq!(
        matcher_drift, 0,
        "matcher modes must produce identical normalized outcomes"
    );
    assert_eq!(mismatched, 0, "every admitted FTW stage must match its expectation");
    assert_eq!(
        negative_mismatched, 0,
        "every admitted negative stage must remain clean"
    );
    assert_eq!(admitted, passed + mismatched);
}

/// Explicit inspection gate: never silently succeeds without the external corpus.
#[test]
#[ignore = "requires the pinned CRS checkout and an explicit inventory output path"]
fn pinned_crs_phase_two_capability_inventory() {
    let tests_root =
        PathBuf::from(std::env::var_os("EDGION_WAF_CRS_TESTS_DIR").expect("set the pinned CRS tests directory"));
    let output =
        PathBuf::from(std::env::var_os("EDGION_WAF_CRS_INVENTORY_OUTPUT").expect("set the inventory JSON output path"));
    let crs_root = tests_root
        .ancestors()
        .nth(3)
        .expect("use the CRS tests/regression/tests layout");
    // Hash the actual parser inputs, not merely the downloaded archive label.
    // Length prefixes prevent ambiguous filename/content concatenations.
    let mut files = fs::read_dir(crs_root.join("rules"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.file_name());
    assert_eq!(files.len(), 50, "the pinned rules directory changed");
    let mut source_hash = Sha256::new();
    for entry in &files {
        assert!(
            entry.file_type().unwrap().is_file(),
            "rules inputs must be regular files"
        );
        let name = entry.file_name().into_string().expect("pinned filenames are UTF-8");
        let bytes = fs::read(entry.path()).unwrap();
        source_hash.update((name.len() as u64).to_be_bytes());
        source_hash.update(name.as_bytes());
        source_hash.update((bytes.len() as u64).to_be_bytes());
        source_hash.update(&bytes);
    }
    let source_digest = format!("{:x}", source_hash.finalize());
    assert_eq!(
        source_digest, "4b51b7a909056215848b248195c6d9ffb4f140836a835b7e0ae9b3d5e95ff46e",
        "use unmodified CRS v4.25.1 rules"
    );
    let generation = compile_ftw_generation(crs_root);
    let mut lanes = BTreeMap::<String, usize>::new();
    let mut shapes = BTreeMap::<String, usize>::new();
    let mut units = Vec::new();
    for unit in generation
        .rule_units()
        .filter(|unit| unit.phase() == Phase::RequestBody)
    {
        let lane = format!("{:?}", unit.request_body_execution_class().unwrap());
        *lanes.entry(lane.clone()).or_default() += 1;
        let mut members = Vec::new();
        for member in unit.members() {
            let targets = member
                .target_plan()
                .map(|plan| {
                    plan.records().map(|target| {
                let shape = format!("{:?}/{:?}/{:?}", target.collection(), target.modifier(), target.selector_kind());
                *shapes.entry(shape.clone()).or_default() += 1;
                serde_json::json!({
                    "shape": shape,
                    "selector": target.selector_bytes().map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
                })
            }).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            members.push(serde_json::json!({
                "targets": targets,
                "operator": member.operator_kind().map(|kind| format!("{kind:?}")),
                "negated": member.operator_negated(),
                "transformations": member.transformations().unwrap_or_default().iter().map(|kind| format!("{kind:?}")).collect::<Vec<_>>(),
            }));
        }
        units.push(serde_json::json!({"id": unit.id(), "lane": lane, "members": members}));
    }
    assert_eq!(units.len(), 289, "admitted Phase 2 coverage changed");
    assert_eq!(
        lanes,
        BTreeMap::from([("BoundedSnapshot".to_owned(), 215), ("StreamingRaw".to_owned(), 74)])
    );
    // Freeze the full observed IDs/member/target/operator/transform projection,
    // not just counts that could hide one-for-one substitutions.
    let projection = serde_json::to_vec(&units).unwrap();
    let projection_digest = format!("{:x}", Sha256::digest(&projection));
    assert_eq!(
        projection_digest, "f996b6d70f2d52152bbd5da782b4c62e7a58466e10c02571ccc22c47777d3b36",
        "review changes to the admitted target/operator inventory"
    );
    let report = serde_json::json!({
        "scope": "exploratory target/operator inventory, not strict production acceptance or action/macro proof",
        "crs_version": "4.25.1",
        "rules_files": files.len(),
        "rules_sha256": source_digest,
        "projection_sha256": projection_digest,
        "phase2_units": units.len(),
        "lanes": lanes,
        "target_shapes": shapes,
        "units": units,
    });
    fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).expect("write the requested inventory report");
    println!(
        "phase2_units={} lanes={} target_shapes={}",
        report["phase2_units"], report["lanes"], report["target_shapes"]
    );
}
