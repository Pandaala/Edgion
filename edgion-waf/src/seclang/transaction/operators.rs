use super::*;

/// Valid only during one borrowed operator stage; never keyed by buffer address.
#[derive(Default)]
pub(super) struct StageScanCache {
    pub(super) storage: ScanCacheStorage,
    #[cfg(test)]
    pub(super) scans: usize,
}

/// Dense small namespaces avoid heap storage; larger namespaces keep the
/// existing per-domain admitted capacity instead of a global bitmap.
pub(super) enum ScanCacheStorage {
    Single(Option<usize>),
    Inline { bits: u64, capacity: usize },
    Overflow(Vec<usize>),
}
impl Default for ScanCacheStorage {
    fn default() -> Self {
        Self::Inline { bits: 0, capacity: 0 }
    }
}
impl StageScanCache {
    pub(super) fn with_storage(mut self, storage: ScanCacheStorage) -> Self {
        self.storage = storage;
        self
    }
    fn bit(group: usize) -> Option<u64> {
        u32::try_from(group).ok().and_then(|shift| 1_u64.checked_shl(shift))
    }
    pub(super) fn contains(&self, group: usize) -> bool {
        match &self.storage {
            ScanCacheStorage::Single(last) => *last == Some(group),
            ScanCacheStorage::Inline { bits, .. } => Self::bit(group).is_some_and(|bit| bits & bit != 0),
            ScanCacheStorage::Overflow(scanned) => scanned.contains(&group),
        }
    }
    pub(super) fn len(&self) -> usize {
        match &self.storage {
            ScanCacheStorage::Single(last) => usize::from(last.is_some()),
            ScanCacheStorage::Inline { bits, .. } => bits.count_ones() as usize,
            ScanCacheStorage::Overflow(scanned) => scanned.len(),
        }
    }
    pub(super) fn heap_capacity(&self) -> usize {
        match &self.storage {
            ScanCacheStorage::Overflow(scanned) => scanned.capacity(),
            _ => 0,
        }
    }
    pub(super) fn full(&self) -> bool {
        match &self.storage {
            ScanCacheStorage::Single(_) => false,
            ScanCacheStorage::Inline { capacity, .. } => self.len() == *capacity,
            ScanCacheStorage::Overflow(scanned) => scanned.len() == scanned.capacity(),
        }
    }
    pub(super) fn insert(&mut self, group: usize) -> Result<(), RuntimeError> {
        if self.contains(group) {
            return Ok(());
        }
        if self.full() {
            return Err(RuntimeError::GenerationInvariant);
        }
        match &mut self.storage {
            ScanCacheStorage::Single(last) => *last = Some(group),
            ScanCacheStorage::Inline { bits, .. } => {
                *bits |= Self::bit(group).ok_or(RuntimeError::GenerationInvariant)?;
            }
            ScanCacheStorage::Overflow(scanned) => scanned.push(group),
        }
        Ok(())
    }
    pub(super) fn clear(&mut self) {
        match &mut self.storage {
            ScanCacheStorage::Single(last) => *last = None,
            ScanCacheStorage::Inline { bits, .. } => *bits = 0,
            ScanCacheStorage::Overflow(scanned) => scanned.clear(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BorrowedTransformKey {
    range: ByteRange,
    plan_id: usize,
    max_output_bytes: usize,
    max_work_bytes: usize,
}

/// One successful identity walk over immutable request bytes. No payload is retained.
#[derive(Default)]
pub(super) struct BorrowedTransformMemo {
    pub(super) key: Option<BorrowedTransformKey>,
}

impl BorrowedTransformMemo {
    pub(super) fn clear(&mut self) {
        self.key = None;
    }

    pub(super) fn prepare(
        &mut self,
        value: SelectedBytes,
        plan_id: Option<usize>,
        request: &RequestState<'_>,
        limits: RuntimeLimits,
    ) -> Option<(&mut Self, BorrowedTransformKey)> {
        let key = (|| {
            let SelectedBytes::Request(range) = value else {
                return None;
            };
            let plan_id = plan_id?;
            let end = range.start.checked_add(range.len)?;
            request.bytes.get(range.start..end)?;
            Some(BorrowedTransformKey {
                range,
                plan_id,
                max_output_bytes: limits.max_transformation_output_bytes,
                max_work_bytes: limits.max_transformation_work_bytes,
            })
        })();
        if key.is_none() {
            self.clear();
        }
        key.map(|key| (self, key))
    }
}

/// One immutable request range/profile and one group's existing result bits.
/// This retains no input bytes and never allocates cache storage.
pub(super) struct OrdinaryScanCache {
    pub(super) key: Option<(ByteRange, Option<usize>)>,
    pub(super) cache: StageScanCache,
}
impl OrdinaryScanCache {
    pub(super) fn new() -> Self {
        Self {
            key: None,
            cache: StageScanCache::default().with_storage(ScanCacheStorage::Single(None)),
        }
    }
    pub(super) fn clear(&mut self) {
        self.key = None;
        self.cache.clear();
    }
    pub(super) fn prepare(
        &mut self,
        value: SelectedBytes,
        profile: Option<usize>,
        eligible: bool,
    ) -> Option<&mut StageScanCache> {
        if let SelectedBytes::Request(range) = value {
            if eligible {
                let key = Some((range, profile));
                if self.key != key {
                    self.cache.clear();
                    self.key = key;
                }
                return Some(&mut self.cache);
            }
        }
        self.clear();
        None
    }
}

pub(super) fn physical_operator_work(
    context: &SingleInputContext<'_>,
    plan: CompiledOperatorPlan,
    len: usize,
) -> Result<usize, RuntimeError> {
    let mut work = len.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
    match plan.operand {
        CompiledOperatorOperand::Matcher {
            group, pattern_count, ..
        } => {
            let cache = context.scan_cache.as_deref().ok_or(RuntimeError::GenerationInvariant)?;
            work = cache
                .len()
                .checked_mul(2)
                .and_then(|v| v.checked_add(pattern_count))
                .and_then(|v| v.checked_add(1))
                .ok_or(RuntimeError::Arithmetic)?;
            if !cache.contains(group) {
                let count = context
                    .generation
                    .matcher_at(group)
                    .ok_or(RuntimeError::GenerationInvariant)?
                    .pattern_count();
                work = work
                    .checked_add(
                        len.checked_add(1)
                            .and_then(|v| v.checked_mul(count.checked_add(1)?))
                            .ok_or(RuntimeError::Arithmetic)?,
                    )
                    .ok_or(RuntimeError::Arithmetic)?;
            }
        }
        CompiledOperatorOperand::String { template, .. }
        | CompiledOperatorOperand::Integer {
            operand: CompiledIntegerOperand::Template(template),
            ..
        } => {
            let template = context
                .generation
                .templates
                .get(template)
                .ok_or(RuntimeError::GenerationInvariant)?;
            work = work
                .checked_add(template.static_bytes)
                .and_then(|v| v.checked_add(template.segment_count.checked_mul(2)?))
                .ok_or(RuntimeError::Arithmetic)?;
            if let CompiledOperatorOperand::String { relation, .. } = plan.operand {
                let (haystack, needle) = match relation {
                    StringRelation::Contains => (len, template.static_bytes),
                    StringRelation::Within => (template.static_bytes, len),
                    _ => (0, 0),
                };
                if needle != 0 && needle <= haystack {
                    let comparisons = (haystack - needle + 1)
                        .checked_mul(needle)
                        .ok_or(RuntimeError::Arithmetic)?;
                    work = work.checked_add(comparisons).ok_or(RuntimeError::Arithmetic)?;
                }
            }
        }
        CompiledOperatorOperand::IpNetworks { count, .. } => {
            work = work.checked_add(count).ok_or(RuntimeError::Arithmetic)?;
        }
        _ => {}
    }
    Ok(work)
}

// Independent inputs: the matcher operand coordinates (group, range), input,
// mode and optional stage cache vary per call; tests drive it with bare slots.
#[expect(
    clippy::too_many_arguments,
    reason = "shared matcher operands and optional stage cache stay explicit"
)]
pub(super) fn evaluate_matcher_operand_cached(
    generation: &CompiledGeneration,
    group_scratch: &mut [Option<MatchScratch>],
    group: usize,
    pattern_start: usize,
    pattern_count: usize,
    input: &[u8],
    evaluation_mode: EvaluationMode,
    cache: Option<&mut StageScanCache>,
) -> Result<bool, RuntimeError> {
    let matcher = generation.matcher_at(group).ok_or(RuntimeError::GenerationInvariant)?;
    let scratch = group_scratch.get_mut(group).ok_or(RuntimeError::GenerationInvariant)?;
    let end = pattern_start
        .checked_add(pattern_count)
        .ok_or(RuntimeError::Arithmetic)?;
    if cache.as_ref().is_some_and(|cache| cache.contains(group)) {
        let scratch = scratch.as_mut().ok_or(RuntimeError::GenerationInvariant)?;
        let result = scratch.contains_any_in_range(matcher.pattern_count(), pattern_start..end);
        #[cfg(test)]
        measurements::record(Event::RangeQueries, 1);
        #[cfg(test)]
        measurements::record(Event::RangeQueryFailures, usize::from(result.is_none()));
        return result.ok_or(RuntimeError::GenerationInvariant);
    }
    // Capacity was admitted by the collector before this callback. No hidden
    // growth is permitted in the canonical matcher seam.
    if cache.as_ref().is_some_and(|cache| cache.full()) {
        return Err(RuntimeError::GenerationInvariant);
    }
    let scratch = lazy_group_scratch(scratch, matcher);
    let matches = match evaluation_mode {
        EvaluationMode::Accelerated => matcher.scan(input, scratch),
        EvaluationMode::Reference => matcher.scan_reference(input, scratch),
    }
    .map_err(map_matcher_error)?;
    #[cfg(test)]
    measurements::record(Event::FullScans, 1);
    let result = matches.contains_any_in_range(pattern_start..end);
    #[cfg(test)]
    measurements::record(Event::RangeQueries, 1);
    #[cfg(test)]
    measurements::record(Event::RangeQueryFailures, usize::from(result.is_none()));
    let result = result.ok_or(RuntimeError::GenerationInvariant)?;
    if let Some(cache) = cache {
        cache.insert(group)?;
        #[cfg(test)]
        {
            cache.scans += 1;
        }
    }
    Ok(result)
}

pub(super) enum OperatorCapture {
    Fingerprint(crate::injection::Fingerprint),
    Phrase(CapturePhraseMatch),
}

pub(super) fn evaluate_operator(
    context: &mut SingleInputContext<'_>,
    plan: CompiledOperatorPlan,
    input: &[u8],
    member_runtime_limit: usize,
    evaluation_mode: EvaluationMode,
) -> Result<(bool, Option<OperatorCapture>), RuntimeError> {
    let generation = context.generation;
    let (request, tx) = (context.request, context.tx);
    let json_budget = context.json_budget.as_deref_mut();
    let mut capture = None;
    let positive = match plan.operand {
        CompiledOperatorOperand::String { relation, template } => {
            let operand = template_operand(generation, request, tx, template, member_runtime_limit, json_budget)?;
            Ok(match relation {
                StringRelation::BeginsWith => input.starts_with(&operand),
                StringRelation::Contains => contains(input, &operand),
                StringRelation::EndsWith => input.ends_with(&operand),
                StringRelation::Equals => input == operand.as_ref(),
                StringRelation::Within => contains(&operand, input),
            })
        }
        CompiledOperatorOperand::Integer { relation, operand } => {
            let expected = match operand {
                CompiledIntegerOperand::Static(value) => value,
                CompiledIntegerOperand::Template(template) => parse_i64(&template_operand(
                    generation,
                    request,
                    tx,
                    template,
                    member_runtime_limit,
                    json_budget,
                )?),
            };
            let actual = parse_i64(input);
            Ok(integer_relation(relation, actual, expected))
        }
        CompiledOperatorOperand::IpNetworks { start, count } => {
            let Some(address) = std::str::from_utf8(input)
                .ok()
                .and_then(|value| value.parse::<std::net::IpAddr>().ok())
            else {
                return Ok((false, None));
            };
            let end = start.checked_add(count).ok_or(RuntimeError::Arithmetic)?;
            let networks = generation
                .ip_networks
                .get(start..end)
                .ok_or(RuntimeError::GenerationInvariant)?;
            Ok(networks.iter().any(|network| ip_matches(address, *network)))
        }
        CompiledOperatorOperand::ByteSet { words } => Ok(input.iter().copied().any(|byte| {
            let index = usize::from(byte);
            words[index / 64] & (1_u64 << (index % 64)) == 0
        })),
        CompiledOperatorOperand::ValidateUrlEncoding => Ok(invalid_url_encoding(input)),
        CompiledOperatorOperand::ValidateUtf8Encoding => Ok(std::str::from_utf8(input).is_err()),
        CompiledOperatorOperand::Unconditional => Ok(true),
        CompiledOperatorOperand::DetectSqli { .. } => {
            capture = crate::injection::detect_sqli(input).map(OperatorCapture::Fingerprint);
            Ok(capture.is_some())
        }
        CompiledOperatorOperand::DetectXss => Ok(crate::injection::detect_xss(input)),
        CompiledOperatorOperand::Regex(reference) => match reference {
            RegexPlanRef::Boolean(program) => {
                let regex = generation
                    .boolean_regexes
                    .get(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let scratch = context
                    .boolean_scratch
                    .get_mut(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                regex
                    .is_match(input, lazy_boolean_scratch(scratch, regex))
                    .map_err(map_regex_scratch_error)
            }
            RegexPlanRef::Capture(program) => {
                #[cfg(test)]
                measurements::record(Event::CaptureBooleanSearches, 1);
                let regex = generation
                    .capture_regexes
                    .get(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let scratch = context
                    .capture_scratch
                    .get_mut(program)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                regex
                    .is_match(input, lazy_capture_scratch(scratch, regex))
                    .map_err(map_regex_scratch_error)
            }
        },
        CompiledOperatorOperand::Matcher {
            group,
            pattern_start,
            pattern_count,
        } => evaluate_matcher_operand_cached(
            generation,
            context.group_scratch,
            group,
            pattern_start,
            pattern_count,
            input,
            evaluation_mode,
            context.scan_cache.as_deref_mut(),
        ),
        CompiledOperatorOperand::CapturePhrase(program) => {
            let matcher = generation
                .capture_phrases
                .get(program)
                .ok_or(RuntimeError::GenerationInvariant)?;
            capture = match evaluation_mode {
                EvaluationMode::Accelerated => matcher.find(input),
                EvaluationMode::Reference => matcher.find_reference(input),
            }
            .map(OperatorCapture::Phrase);
            Ok(capture.is_some())
        }
    }?;
    Ok((positive, capture))
}

pub(super) fn static_template_operand(
    generation: &CompiledGeneration,
    template_id: usize,
    member_limit: usize,
) -> Result<Option<&[u8]>, RuntimeError> {
    let template = generation
        .templates
        .get(template_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let end = template
        .segment_start
        .checked_add(template.segment_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let segments = generation
        .segments
        .get(template.segment_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    if segments
        .iter()
        .any(|segment| matches!(segment.kind, CompiledSegmentKind::Macro(_)))
    {
        return Ok(None);
    }
    let limit = template.runtime_byte_limit.min(member_limit);
    let mut start = None;
    let mut len = 0usize;
    for segment in segments {
        let CompiledSegmentKind::Literal(range) = segment.kind else {
            return Err(RuntimeError::GenerationInvariant);
        };
        let bytes = generation_bytes(&generation.bytes, range)?;
        let base = *start.get_or_insert(range.start);
        if base.checked_add(len).ok_or(RuntimeError::Arithmetic)? != range.start {
            return Err(RuntimeError::GenerationInvariant);
        }
        len = checked_runtime_append(len, bytes.len(), limit)?;
    }
    match start {
        Some(start) => generation_bytes(&generation.bytes, CompilerByteRange { start, len }).map(Some),
        None => Ok(Some(&[])),
    }
}

pub(super) fn template_operand<'g>(
    generation: &'g CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    template: usize,
    limit: usize,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Cow<'g, [u8]>, RuntimeError> {
    match static_template_operand(generation, template, limit)? {
        Some(value) => Ok(Cow::Borrowed(value)),
        None => expand_template::<true>(generation, request, tx, template, limit, json_budget).map(Cow::Owned),
    }
}

// Validation still resolves and meters every macro, but need not retain its bytes.
pub(super) fn expand_template<const RETAIN_OUTPUT: bool>(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    template_id: usize,
    member_limit: usize,
    mut json_budget: Option<&mut InspectionBudget>,
) -> Result<Vec<u8>, RuntimeError> {
    let template = *generation
        .templates
        .get(template_id)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let limit = template.runtime_byte_limit.min(member_limit);
    let end = template
        .segment_start
        .checked_add(template.segment_count)
        .ok_or(RuntimeError::Arithmetic)?;
    let segments = generation
        .segments
        .get(template.segment_start..end)
        .ok_or(RuntimeError::GenerationInvariant)?;
    let mut output = Vec::new();
    if RETAIN_OUTPUT {
        output
            .try_reserve(template.static_bytes.min(limit))
            .map_err(|_| RuntimeError::Allocation)?;
    }
    let mut expanded_len = 0_usize;
    for segment in segments {
        let value = match segment.kind {
            CompiledSegmentKind::Literal(range) => generation_bytes(&generation.bytes, range)?,
            CompiledSegmentKind::Macro(macro_id) => {
                let reference = *generation
                    .macros
                    .get(macro_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let key = generation_bytes(&generation.bytes, reference.key)?;
                let value = macro_value(
                    generation,
                    macro_id,
                    request,
                    tx,
                    reference.source,
                    key,
                    json_budget.as_deref_mut(),
                )?;
                value.unwrap_or_default()
            }
        };
        expanded_len = checked_runtime_append(expanded_len, value.len(), limit)?;
        if RETAIN_OUTPUT {
            output.extend_from_slice(value);
        }
    }
    Ok(output)
}

fn macro_value<'request>(
    generation: &CompiledGeneration,
    macro_id: usize,
    request: &'request RequestState,
    tx: &'request TxState,
    source: MacroSource,
    key: &[u8],
    mut json_budget: Option<&mut InspectionBudget>,
) -> Result<Option<&'request [u8]>, RuntimeError> {
    Ok(match source {
        MacroSource::Args => {
            if let Some(value) = request.first_named(CollectionKind::Args, key, false) {
                Some(value)
            } else if let Some(results) = request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .and_then(|body| body.results.as_ref())
            {
                let demand = generation
                    .json_inspection
                    .macro_demands
                    .get(macro_id)
                    .copied()
                    .flatten()
                    .ok_or(RuntimeError::JsonInspectionState)?;
                results
                    .named_value(json_budget.ok_or(RuntimeError::JsonInspectionState)?, demand)
                    .map_err(map_store_error)?
            } else {
                None
            }
        }
        MacroSource::RequestHeaders => request.first_named(CollectionKind::RequestHeaders, key, true),
        MacroSource::RemoteAddr => request.first_value(CollectionKind::RemoteAddr),
        MacroSource::RequestLine => request.first_value(CollectionKind::RequestLine),
        MacroSource::Tx => tx
            .get(key, request, json_budget.as_deref_mut())?
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
        MacroSource::MatchedVar => tx
            .matched_var
            .as_ref()
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
        MacroSource::MatchedVarName => tx
            .matched_var_name
            .as_ref()
            .map(|value| value.bytes(request, json_budget))
            .transpose()?,
    })
}

pub(super) fn generation_bytes(bytes: &[u8], range: CompilerByteRange) -> Result<&[u8], RuntimeError> {
    let end = range.start.checked_add(range.len).ok_or(RuntimeError::Arithmetic)?;
    bytes.get(range.start..end).ok_or(RuntimeError::GenerationInvariant)
}

fn checked_runtime_append(current: usize, added: usize, limit: usize) -> Result<usize, RuntimeError> {
    let actual = current.checked_add(added).ok_or(RuntimeError::Arithmetic)?;
    if actual > limit {
        return Err(RuntimeError::ValueByteLimit { actual, limit });
    }
    Ok(actual)
}

/// Converts a runtime numeric operand with ModSecurity v3's `atoll` semantics.
///
/// Leading C-locale ASCII whitespace (space, `\t`, `\n`, `\v`, `\f`, `\r`) is
/// skipped, then the value is read as an optional `+`/`-` sign followed by the
/// longest leading run of ASCII decimal digits. A value without such a run is
/// `0`, bytes after the run are ignored, and a run outside the `i64` range
/// saturates at `i64::MAX`/`i64::MIN` (glibc `strtoll`). The conversion is
/// total, so client-controlled operands and TX values can never abort a
/// transaction; compile-time constants keep their strict grammar.
pub(super) fn parse_i64(value: &[u8]) -> i64 {
    let start = value
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .unwrap_or(value.len());
    let value = &value[start..];
    let (negative, digits) = match value.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, value),
    };
    let mut result = 0_i64;
    for &byte in digits {
        if !byte.is_ascii_digit() {
            break;
        }
        let digit = i64::from(byte - b'0');
        // Accumulate toward the sign so that `i64::MIN` stays representable.
        let next = result.checked_mul(10).and_then(|scaled| {
            if negative {
                scaled.checked_sub(digit)
            } else {
                scaled.checked_add(digit)
            }
        });
        match next {
            Some(next) => result = next,
            // Further digits cannot leave the saturated value.
            None => return if negative { i64::MIN } else { i64::MAX },
        }
    }
    result
}

pub(super) fn parse_usize(value: &[u8]) -> Result<usize, RuntimeError> {
    let text = std::str::from_utf8(value).map_err(|_| RuntimeError::GenerationInvariant)?;
    // Unlike usize parsing, this internal decimal representation rejects signs.
    if text.starts_with('+') {
        return Err(RuntimeError::GenerationInvariant);
    }
    text.parse().map_err(|_| RuntimeError::GenerationInvariant)
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

pub(super) fn ip_matches(address: std::net::IpAddr, network: CompiledIpNetwork) -> bool {
    match (address, network.address) {
        (std::net::IpAddr::V4(address), std::net::IpAddr::V4(base)) => {
            let prefix = u32::from(network.prefix.min(32));
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            u32::from(address) & mask == u32::from(base)
        }
        (std::net::IpAddr::V6(address), std::net::IpAddr::V6(base)) => {
            let prefix = u32::from(network.prefix.min(128));
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            u128::from(address) & mask == u128::from(base)
        }
        _ => false,
    }
}

pub(super) fn invalid_url_encoding(input: &[u8]) -> bool {
    let mut index = 0_usize;
    while index < input.len() {
        let Some(offset) = memchr::memchr(b'%', &input[index..]) else {
            return false;
        };
        index += offset;
        if index + 2 >= input.len() || hex(input[index + 1]).is_none() || hex(input[index + 2]).is_none() {
            return true;
        }
        index += 3;
    }
    false
}

pub(super) fn map_transform_error(error: TransformError) -> RuntimeError {
    let kind = match error {
        TransformError::OutputLimit { .. } => RuntimeTransformError::OutputLimit,
        TransformError::WorkLimit { .. } => RuntimeTransformError::WorkLimit,
        TransformError::Allocation => RuntimeTransformError::Allocation,
        TransformError::Arithmetic => RuntimeTransformError::Arithmetic,
    };
    RuntimeError::Transformation { kind }
}

pub(super) fn map_regex_scratch_error(error: RegexScratchError) -> RuntimeError {
    match error {
        RegexScratchError::ScratchShape => RuntimeError::Matcher {
            kind: RuntimeMatcherError::ScratchShape,
        },
    }
}

pub(super) fn map_matcher_error(error: MatcherScanError) -> RuntimeError {
    let kind = match error {
        MatcherScanError::ScratchShape => RuntimeMatcherError::ScratchShape,
        MatcherScanError::PhraseSearchMode | MatcherScanError::InvalidPatternId => RuntimeMatcherError::PatternShape,
    };
    RuntimeError::Matcher { kind }
}
