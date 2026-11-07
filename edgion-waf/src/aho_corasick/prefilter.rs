use std::mem::size_of;

use super::{AccelerationKind, BuildError, Match, MatchKind, PatternId};

const MAX_FILTER_BYTES: usize = 3;
const MAX_PACKED_PATTERNS: usize = 16;
const MAX_PACKED_PATTERN_LEN: usize = 64;
const MAX_PACKED_PATTERN_BYTES: usize = 1_024;
pub(super) const TWO_GRAM_TABLE_LEN: usize = 1 << 16;
const TWO_GRAM_MAX_WINDOW_LEN: usize = u8::BITS as usize + 1;
const TWO_GRAM_MAX_CANDIDATE_RATE_Q32: u64 = 1_u64 << 28;
const ADAPT_MIN_ATTEMPTS: u16 = 32;
const ADAPT_MIN_SKIP_PER_ATTEMPT: usize = 4;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchState {
    attempts: u16,
    scanned_bytes: usize,
    skipped_bytes: usize,
    disabled: bool,
}

impl SearchState {
    pub(super) const fn diagnostics(self) -> super::SearchDiagnostics {
        super::SearchDiagnostics {
            candidate_attempts: self.attempts,
            candidate_scanned_bytes: self.scanned_bytes,
            skipped_bytes: self.skipped_bytes,
            prefilter_disabled: self.disabled,
        }
    }

    fn record_candidate(&mut self, at: usize, candidate: usize, scanned: usize) {
        self.attempts = self.attempts.saturating_add(1);
        self.scanned_bytes = self.scanned_bytes.saturating_add(scanned);
        self.skipped_bytes = self.skipped_bytes.saturating_add(candidate.saturating_sub(at));
        if self.attempts >= ADAPT_MIN_ATTEMPTS
            && self.skipped_bytes < usize::from(self.attempts).saturating_mul(ADAPT_MIN_SKIP_PER_ATTEMPT)
        {
            self.disabled = true;
        }
    }

    fn record_none(&mut self, scanned: usize) {
        self.attempts = self.attempts.saturating_add(1);
        self.scanned_bytes = self.scanned_bytes.saturating_add(scanned);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Candidate {
    None,
    Possible { start: usize, scanned: usize },
    Exact { matched: Match, scanned: usize },
}

#[derive(Debug)]
struct StartBytes {
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct RareBytes {
    bytes: Vec<u8>,
    max_offset: u16,
}

#[derive(Debug, Clone, Copy)]
struct PackedEntry {
    fingerprint: u16,
    pattern_id: PatternId,
    start: u32,
    len: u16,
}

#[derive(Debug)]
struct Packed {
    entries: Vec<PackedEntry>,
    pattern_bytes: Vec<u8>,
    match_kind: MatchKind,
}

#[derive(Debug)]
struct TwoGram {
    table: Box<[u8]>,
    window_len: u8,
    hit_bit: u8,
}

#[derive(Debug)]
enum AccelerationInner {
    None,
    Start(StartBytes),
    Rare(RareBytes),
    Packed(Packed),
    TwoGram(TwoGram),
}

/// Immutable conservative acceleration attached above the exact backend.
#[derive(Debug)]
pub(super) struct Acceleration {
    inner: AccelerationInner,
    memory_usage: usize,
}

impl Acceleration {
    pub(super) const fn none() -> Self {
        Self {
            inner: AccelerationInner::None,
            memory_usage: 0,
        }
    }

    pub(super) const fn kind(&self) -> AccelerationKind {
        match self.inner {
            AccelerationInner::None => AccelerationKind::None,
            AccelerationInner::Start(_) => AccelerationKind::StartBytes,
            AccelerationInner::Rare(_) => AccelerationKind::RareBytes,
            AccelerationInner::Packed(_) => AccelerationKind::Packed,
            AccelerationInner::TwoGram(_) => AccelerationKind::TwoGram,
        }
    }

    pub(super) const fn memory_usage(&self) -> usize {
        self.memory_usage
    }

    pub(super) const fn is_none(&self) -> bool {
        matches!(self.inner, AccelerationInner::None)
    }

    pub(super) fn find_candidate(
        &self,
        haystack: &[u8],
        at: usize,
        ascii_case_insensitive: bool,
        state: &mut SearchState,
    ) -> Candidate {
        if state.disabled || at >= haystack.len() {
            return Candidate::Possible { start: at, scanned: 0 };
        }
        let candidate = match &self.inner {
            AccelerationInner::None => Candidate::Possible { start: at, scanned: 0 },
            AccelerationInner::Start(filter) => filter.find(haystack, at, ascii_case_insensitive),
            AccelerationInner::Rare(filter) => filter.find(haystack, at, ascii_case_insensitive),
            AccelerationInner::Packed(filter) => filter.find(haystack, at, ascii_case_insensitive),
            AccelerationInner::TwoGram(filter) => filter.find(haystack, at, ascii_case_insensitive),
        };
        match candidate {
            Candidate::None => state.record_none(haystack.len().saturating_sub(at)),
            Candidate::Possible { start, scanned } => state.record_candidate(at, start, scanned),
            Candidate::Exact { matched, scanned } => state.record_candidate(at, matched.start(), scanned),
        }
        candidate
    }
}

impl StartBytes {
    fn find(&self, haystack: &[u8], at: usize, ascii_case_insensitive: bool) -> Candidate {
        if let Some(relative) = find_any_byte(&haystack[at..], &self.bytes, ascii_case_insensitive) {
            return Candidate::Possible {
                start: at + relative,
                scanned: relative + 1,
            };
        }
        Candidate::None
    }
}

impl RareBytes {
    fn find(&self, haystack: &[u8], at: usize, ascii_case_insensitive: bool) -> Candidate {
        if let Some(relative) = find_any_byte(&haystack[at..], &self.bytes, ascii_case_insensitive) {
            let position = at + relative;
            return Candidate::Possible {
                start: position.saturating_sub(usize::from(self.max_offset)).max(at),
                scanned: relative + 1,
            };
        }
        Candidate::None
    }
}

fn find_any_byte(haystack: &[u8], needles: &[u8], ascii_case_insensitive: bool) -> Option<usize> {
    if ascii_case_insensitive {
        return haystack
            .iter()
            .copied()
            .position(|byte| needles.contains(&byte.to_ascii_lowercase()));
    }
    if haystack.first().is_some_and(|byte| needles.contains(byte)) {
        return Some(0);
    }
    let prefix_len = haystack.len().min(size_of::<u64>());
    if let Some(relative) = haystack[1.min(prefix_len)..prefix_len]
        .iter()
        .copied()
        .position(|byte| needles.contains(&byte))
    {
        return Some(relative + 1);
    }
    let remainder = &haystack[prefix_len..];
    let (chunks, tail) = remainder.as_chunks::<{ size_of::<u64>() }>();
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        let word = u64::from_ne_bytes(*chunk);
        let possible = needles
            .iter()
            .copied()
            .any(|needle| has_zero_byte(word ^ (u64::from(needle) * U64_ONES)));
        if possible {
            let relative = chunk
                .iter()
                .copied()
                .position(|byte| needles.contains(&byte))
                .expect("the word mask reported a matching byte");
            return Some(prefix_len + chunk_index * size_of::<u64>() + relative);
        }
    }
    let remainder_start = haystack.len() - tail.len();
    tail.iter()
        .copied()
        .position(|byte| needles.contains(&byte))
        .map(|relative| remainder_start + relative)
}

const U64_ONES: u64 = u64::MAX / 0xFF;
const U64_HIGHS: u64 = U64_ONES * 0x80;

const fn has_zero_byte(word: u64) -> bool {
    word.wrapping_sub(U64_ONES) & !word & U64_HIGHS != 0
}

impl Packed {
    fn find(&self, haystack: &[u8], at: usize, ascii_case_insensitive: bool) -> Candidate {
        if haystack.len().saturating_sub(at) < 2 {
            return Candidate::None;
        }
        for start in at..haystack.len() - 1 {
            let fingerprint = fingerprint(
                fold_ascii(haystack[start], ascii_case_insensitive),
                fold_ascii(haystack[start + 1], ascii_case_insensitive),
            );
            let first = self.entries.partition_point(|entry| entry.fingerprint < fingerprint);
            for entry in self.entries[first..]
                .iter()
                .take_while(|entry| entry.fingerprint == fingerprint)
            {
                if self.matches_at(*entry, haystack, start, ascii_case_insensitive) {
                    let matched = Match::from_validated(entry.pattern_id, start, start + usize::from(entry.len));
                    let scanned = start - at + 2;
                    return if self.match_kind == MatchKind::Standard {
                        Candidate::Possible { start, scanned }
                    } else {
                        Candidate::Exact { matched, scanned }
                    };
                }
            }
        }
        Candidate::None
    }

    fn matches_at(&self, entry: PackedEntry, haystack: &[u8], start: usize, ascii_case_insensitive: bool) -> bool {
        let len = usize::from(entry.len);
        let Some(candidate) = haystack.get(start..start.saturating_add(len)) else {
            return false;
        };
        let pattern_start = entry.start as usize;
        let pattern = &self.pattern_bytes[pattern_start..pattern_start + len];
        if ascii_case_insensitive {
            candidate
                .iter()
                .copied()
                .map(|byte| byte.to_ascii_lowercase())
                .eq(pattern.iter().copied())
        } else {
            candidate == pattern
        }
    }
}

impl TwoGram {
    fn find(&self, haystack: &[u8], at: usize, ascii_case_insensitive: bool) -> Candidate {
        let window_len = usize::from(self.window_len);
        if haystack.len().saturating_sub(at) < window_len {
            return Candidate::None;
        }
        let mut previous = fold_ascii(haystack[at], ascii_case_insensitive);
        let mut state = u8::MAX;
        for (relative, source) in haystack[at + 1..].iter().copied().enumerate() {
            let current = fold_ascii(source, ascii_case_insensitive);
            state = (state << 1) | self.table[usize::from(fingerprint(previous, current))];
            let boundary = at + relative + 2;
            if state & self.hit_bit == 0 {
                return Candidate::Possible {
                    start: boundary.saturating_sub(window_len).max(at),
                    scanned: boundary - at,
                };
            }
            previous = current;
        }
        Candidate::None
    }
}

/// Bounded build-time profile collected in the validated pattern pass.
#[derive(Debug)]
pub(super) struct AccelerationBuilder {
    enabled: bool,
    ascii_case_insensitive: bool,
    match_kind: MatchKind,
    has_empty: bool,
    start_set: [bool; 256],
    start_count: usize,
    rare_set: [bool; 256],
    rare_offsets: [u16; 256],
    rare_count: usize,
    rare_available: bool,
    packed_available: bool,
    packed_entries: Vec<PackedEntry>,
    packed_pattern_bytes: Vec<u8>,
    two_gram_table: Option<Vec<u8>>,
    two_gram_min_pattern_len: usize,
    two_gram_available: bool,
}

impl AccelerationBuilder {
    pub(super) fn new(enabled: bool, ascii_case_insensitive: bool, match_kind: MatchKind) -> Self {
        Self {
            enabled,
            ascii_case_insensitive,
            match_kind,
            has_empty: false,
            start_set: [false; 256],
            start_count: 0,
            rare_set: [false; 256],
            rare_offsets: [0; 256],
            rare_count: 0,
            rare_available: true,
            packed_available: true,
            packed_entries: Vec::new(),
            packed_pattern_bytes: Vec::new(),
            two_gram_table: None,
            two_gram_min_pattern_len: usize::MAX,
            two_gram_available: true,
        }
    }

    pub(super) fn add(&mut self, pattern_id: PatternId, pattern: &[u8]) -> Result<(), BuildError> {
        if !self.enabled {
            return Ok(());
        }
        if pattern.is_empty() {
            self.has_empty = true;
            self.disable_packed();
            return Ok(());
        }

        let first = fold_ascii(pattern[0], self.ascii_case_insensitive);
        if !self.start_set[usize::from(first)] {
            self.start_set[usize::from(first)] = true;
            self.start_count += 1;
        }
        self.add_rare(pattern);
        self.add_packed(pattern_id, pattern)?;
        self.add_two_gram(pattern)?;
        Ok(())
    }

    pub(super) fn build(mut self) -> Result<Acceleration, BuildError> {
        if !self.enabled || self.has_empty {
            return Ok(Acceleration::none());
        }
        if self.match_kind != MatchKind::Standard
            && self.packed_available
            && self.packed_entries.len() >= 2
            && self.packed_entries.len() <= MAX_PACKED_PATTERNS
        {
            return self.build_packed();
        }
        if (1..MAX_FILTER_BYTES).contains(&self.start_count) {
            return self.build_start();
        }
        if self.rare_available && (1..=MAX_FILTER_BYTES).contains(&self.rare_count) {
            return self.build_rare();
        }
        if self.packed_available && self.packed_entries.len() >= 2 && self.packed_entries.len() <= MAX_PACKED_PATTERNS {
            return self.build_packed();
        }
        if self.start_count == MAX_FILTER_BYTES {
            return self.build_start();
        }
        if self.two_gram_available && self.two_gram_table.is_some() {
            return self.build_two_gram();
        }
        Ok(Acceleration::none())
    }

    fn add_two_gram(&mut self, pattern: &[u8]) -> Result<(), BuildError> {
        if !self.two_gram_available {
            return Ok(());
        }
        if pattern.len() < 2 {
            self.two_gram_available = false;
            self.two_gram_table = None;
            return Ok(());
        }
        let table = match &mut self.two_gram_table {
            Some(table) => table,
            None => {
                let mut table = Vec::new();
                table
                    .try_reserve_exact(TWO_GRAM_TABLE_LEN)
                    .map_err(|_| BuildError::Allocation {
                        component: "two-gram acceleration table",
                    })?;
                table.resize(TWO_GRAM_TABLE_LEN, u8::MAX);
                self.two_gram_table.insert(table)
            }
        };
        self.two_gram_min_pattern_len = self.two_gram_min_pattern_len.min(pattern.len());
        let window_len = pattern.len().min(TWO_GRAM_MAX_WINDOW_LEN);
        for position in 0..window_len - 1 {
            let first = fold_ascii(pattern[position], self.ascii_case_insensitive);
            let second = fold_ascii(pattern[position + 1], self.ascii_case_insensitive);
            table[usize::from(fingerprint(first, second))] &= !(1_u8 << position);
        }
        Ok(())
    }

    fn add_rare(&mut self, pattern: &[u8]) {
        if !self.rare_available || pattern.len() > usize::from(u16::MAX) + 1 {
            self.rare_available = false;
            return;
        }
        let mut selected = None;
        for (offset, source_byte) in pattern.iter().copied().enumerate() {
            let byte = fold_ascii(source_byte, self.ascii_case_insensitive);
            let candidate = (frequency_rank(byte), offset, byte);
            if selected.is_none_or(|current| candidate < current) {
                selected = Some(candidate);
            }
        }
        let Some((_, offset, byte)) = selected else {
            return;
        };
        let index = usize::from(byte);
        if !self.rare_set[index] {
            self.rare_set[index] = true;
            self.rare_count += 1;
        }
        self.rare_offsets[index] = self.rare_offsets[index].max(offset as u16);
        if self.rare_count > MAX_FILTER_BYTES {
            self.rare_available = false;
        }
    }

    fn add_packed(&mut self, pattern_id: PatternId, pattern: &[u8]) -> Result<(), BuildError> {
        if !self.packed_available {
            return Ok(());
        }
        let next_count = self.packed_entries.len() + 1;
        let next_bytes = self.packed_pattern_bytes.len().saturating_add(pattern.len());
        if next_count > MAX_PACKED_PATTERNS
            || !(2..=MAX_PACKED_PATTERN_LEN).contains(&pattern.len())
            || next_bytes > MAX_PACKED_PATTERN_BYTES
        {
            self.disable_packed();
            return Ok(());
        }
        self.packed_entries.try_reserve(1).map_err(|_| BuildError::Allocation {
            component: "packed acceleration entries",
        })?;
        self.packed_pattern_bytes
            .try_reserve(pattern.len())
            .map_err(|_| BuildError::Allocation {
                component: "packed acceleration pattern bytes",
            })?;
        let start = u32::try_from(self.packed_pattern_bytes.len()).map_err(|_| BuildError::IndexOverflow {
            component: "packed pattern start",
            value: self.packed_pattern_bytes.len(),
        })?;
        let len = u16::try_from(pattern.len()).map_err(|_| BuildError::IndexOverflow {
            component: "packed pattern length",
            value: pattern.len(),
        })?;
        let first = fold_ascii(pattern[0], self.ascii_case_insensitive);
        let second = fold_ascii(pattern[1], self.ascii_case_insensitive);
        self.packed_entries.push(PackedEntry {
            fingerprint: fingerprint(first, second),
            pattern_id,
            start,
            len,
        });
        self.packed_pattern_bytes.extend(
            pattern
                .iter()
                .copied()
                .map(|byte| fold_ascii(byte, self.ascii_case_insensitive)),
        );
        Ok(())
    }

    fn disable_packed(&mut self) {
        self.packed_available = false;
        self.packed_entries.clear();
        self.packed_pattern_bytes.clear();
    }

    fn build_start(&self) -> Result<Acceleration, BuildError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.start_count)
            .map_err(|_| BuildError::Allocation {
                component: "start-byte acceleration",
            })?;
        bytes.extend((u8::MIN..=u8::MAX).filter(|byte| self.start_set[usize::from(*byte)]));
        let memory_usage = bytes.capacity();
        Ok(Acceleration {
            inner: AccelerationInner::Start(StartBytes { bytes }),
            memory_usage,
        })
    }

    fn build_rare(&self) -> Result<Acceleration, BuildError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.rare_count)
            .map_err(|_| BuildError::Allocation {
                component: "rare-byte acceleration",
            })?;
        bytes.extend((u8::MIN..=u8::MAX).filter(|&byte| self.rare_set[usize::from(byte)]));
        let memory_usage = bytes.capacity().saturating_mul(size_of::<u8>());
        let max_offset = self.rare_offsets.iter().copied().max().unwrap_or(0);
        Ok(Acceleration {
            inner: AccelerationInner::Rare(RareBytes { bytes, max_offset }),
            memory_usage,
        })
    }

    fn build_packed(&mut self) -> Result<Acceleration, BuildError> {
        self.packed_entries.sort_by(|left, right| {
            left.fingerprint
                .cmp(&right.fingerprint)
                .then_with(|| match self.match_kind {
                    MatchKind::Standard | MatchKind::LeftmostFirst => left.pattern_id.cmp(&right.pattern_id),
                    MatchKind::LeftmostLongest => right
                        .len
                        .cmp(&left.len)
                        .then_with(|| left.pattern_id.cmp(&right.pattern_id)),
                })
        });
        let memory_usage = self
            .packed_entries
            .capacity()
            .saturating_mul(size_of::<PackedEntry>())
            .saturating_add(self.packed_pattern_bytes.capacity());
        Ok(Acceleration {
            inner: AccelerationInner::Packed(Packed {
                entries: std::mem::take(&mut self.packed_entries),
                pattern_bytes: std::mem::take(&mut self.packed_pattern_bytes),
                match_kind: self.match_kind,
            }),
            memory_usage,
        })
    }

    fn build_two_gram(&mut self) -> Result<Acceleration, BuildError> {
        let window_len = self.two_gram_min_pattern_len.min(TWO_GRAM_MAX_WINDOW_LEN);
        let Some(table) = self.two_gram_table.take() else {
            return Ok(Acceleration::none());
        };
        if window_len < 2 {
            return Ok(Acceleration::none());
        }
        let mut zero_counts = [0_u32; TWO_GRAM_MAX_WINDOW_LEN - 1];
        for mask in table.iter().copied() {
            for (position, count) in zero_counts[..window_len - 1].iter_mut().enumerate() {
                *count += u32::from(mask & (1_u8 << position) == 0);
            }
        }
        let mut rate_q32 = 1_u64 << 32;
        for count in zero_counts[..window_len - 1].iter().copied() {
            rate_q32 = rate_q32
                .saturating_mul(u64::from(count))
                .div_ceil(TWO_GRAM_TABLE_LEN as u64);
        }
        if rate_q32 >= TWO_GRAM_MAX_CANDIDATE_RATE_Q32 {
            return Ok(Acceleration::none());
        }
        let table = table.into_boxed_slice();
        Ok(Acceleration {
            memory_usage: table.len(),
            inner: AccelerationInner::TwoGram(TwoGram {
                table,
                window_len: window_len as u8,
                hit_bit: 1_u8 << (window_len - 2),
            }),
        })
    }
}

fn fingerprint(first: u8, second: u8) -> u16 {
    u16::from(first) << 8 | u16::from(second)
}

fn fold_ascii(byte: u8, ascii_case_insensitive: bool) -> u8 {
    if ascii_case_insensitive {
        byte.to_ascii_lowercase()
    } else {
        byte
    }
}

fn frequency_rank(byte: u8) -> u8 {
    match byte {
        b' ' | b'\t' | b'\r' | b'\n' => 3,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => 2,
        0x20..=0x7E => 1,
        _ => 0,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/aho_corasick/prefilter_test.rs"]
mod tests;
