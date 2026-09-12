use std::collections::VecDeque;

use bytes::Bytes;

use crate::plugins::runtime::traits::session::BodyReadError;

/// Maximum number of immutable segments retained by one logical request body.
pub const MAX_BODY_SEGMENTS: usize = 4_096;

/// Result of a body operation together with the raw source bytes fetched and
/// charged to its scan budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyScanResult<T> {
    pub value: T,
    pub bytes_scanned: u64,
}

/// Errors produced by request-body operations before session policy mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyOperationError {
    Body(BodyReadError),
    ScanLimitExceeded { limit: u64 },
    SegmentLimitExceeded { limit: usize },
    InvalidRange,
    EmptyPattern,
}

impl From<BodyReadError> for BodyOperationError {
    fn from(value: BodyReadError) -> Self {
        Self::Body(value)
    }
}

impl std::fmt::Display for BodyOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Body(error) => error.fmt(f),
            Self::ScanLimitExceeded { limit } => {
                write!(f, "request body scan exceeds limit of {limit} bytes")
            }
            Self::SegmentLimitExceeded { limit } => {
                write!(f, "request body mutation exceeds segment limit of {limit}")
            }
            Self::InvalidRange => write!(f, "invalid request body range"),
            Self::EmptyPattern => write!(f, "request body replacement pattern is empty"),
        }
    }
}

impl std::error::Error for BodyOperationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanControl {
    Continue,
    Stop,
}

/// Owned raw-byte scanner. Disk-backed scans move the scanner into one blocking
/// job, so implementations must not borrow VM or session state.
pub trait RawBodyScanner: Send + 'static {
    type Output: Send + 'static;

    /// Whether scanner callbacks may block and therefore require execution on
    /// the blocking pool even for a small memory-backed body.
    fn requires_blocking_context(&self) -> bool {
        false
    }

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError>;
    fn finish(self) -> Result<Self::Output, BodyOperationError>;
}

/// Owned logical-text scanner. Input chunks are valid UTF-8 and together have
/// exactly the semantics of `String::from_utf8_lossy` over the complete body.
pub trait TextBodyScanner: Send + 'static {
    type Output: Send + 'static;

    fn push(&mut self, chunk: &str) -> Result<ScanControl, BodyOperationError>;
    fn finish(self) -> Result<Self::Output, BodyOperationError>;
}

/// A bounded text read either returns the complete selected range or no text.
/// Output overflow is a scan outcome so fetched-byte accounting is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundedTextResult {
    Complete(String),
    OutputLimitExceeded,
}

pub(crate) struct BoundedTextCollector {
    text: String,
    limit: usize,
    exceeded: bool,
}

impl BoundedTextCollector {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
            exceeded: false,
        }
    }

    fn reserve_output(&mut self, required: usize) -> Result<(), BodyOperationError> {
        if required > self.text.capacity() {
            // Grow geometrically, but never request a capacity above the
            // output limit. The allocator may round this requested capacity.
            let target = Self::growth_target(self.text.capacity(), required, self.limit);
            self.text
                .try_reserve_exact(target - self.text.len())
                .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        }
        Ok(())
    }

    fn growth_target(current_capacity: usize, required: usize, limit: usize) -> usize {
        required.max(current_capacity.saturating_mul(2)).min(limit)
    }
}

impl TextBodyScanner for BoundedTextCollector {
    type Output = BoundedTextResult;

    fn push(&mut self, chunk: &str) -> Result<ScanControl, BodyOperationError> {
        if self.exceeded {
            return Ok(ScanControl::Stop);
        }
        let Some(required) = self
            .text
            .len()
            .checked_add(chunk.len())
            .filter(|len| *len <= self.limit)
        else {
            self.exceeded = true;
            self.text = String::new();
            return Ok(ScanControl::Stop);
        };
        self.reserve_output(required)?;
        self.text.push_str(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(if self.exceeded {
            BoundedTextResult::OutputLimitExceeded
        } else {
            BoundedTextResult::Complete(self.text)
        })
    }
}

/// Adapts a text scanner to raw body chunks without materializing the body.
pub struct LossyTextScanner<S> {
    inner: S,
    pending: [u8; 4],
    pending_len: usize,
    stopped: bool,
}

impl<S> LossyTextScanner<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            pending: [0; 4],
            pending_len: 0,
            stopped: false,
        }
    }

    fn feed_lossy(&mut self, mut bytes: &[u8], final_chunk: bool) -> Result<ScanControl, BodyOperationError>
    where
        S: TextBodyScanner,
    {
        if self.stopped {
            return Ok(ScanControl::Stop);
        }

        // Resolve the incomplete scalar retained from the prior chunk by
        // adding at most the bytes needed to make one UTF-8 decision. The
        // remaining chunk is scanned directly and is never copied here.
        while self.pending_len > 0 {
            match std::str::from_utf8(&self.pending[..self.pending_len]) {
                Ok(valid) => {
                    if !valid.is_empty() && self.inner.push(valid)? == ScanControl::Stop {
                        self.stopped = true;
                    }
                    self.pending_len = 0;
                }
                Err(error) => {
                    let valid_end = error.valid_up_to();
                    if valid_end > 0 {
                        let valid = std::str::from_utf8(&self.pending[..valid_end])
                            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                        if self.inner.push(valid)? == ScanControl::Stop {
                            self.stopped = true;
                        }
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            if !self.stopped && self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                            }
                            let consumed = valid_end + invalid_len;
                            self.pending.copy_within(consumed..self.pending_len, 0);
                            self.pending_len -= consumed;
                        }
                        None if final_chunk => {
                            if !self.stopped && self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                            }
                            self.pending_len = 0;
                        }
                        None => {
                            if let Some((&next, remaining)) = bytes.split_first() {
                                if self.pending_len == self.pending.len() {
                                    return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                                }
                                self.pending[self.pending_len] = next;
                                self.pending_len += 1;
                                bytes = remaining;
                            } else {
                                return Ok(ScanControl::Continue);
                            }
                        }
                    }
                }
            }
            if self.stopped {
                return Ok(ScanControl::Stop);
            }
        }

        while !bytes.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(valid) => {
                    if !valid.is_empty() && self.inner.push(valid)? == ScanControl::Stop {
                        self.stopped = true;
                    }
                    break;
                }
                Err(error) => {
                    let valid_end = error.valid_up_to();
                    if valid_end > 0 {
                        let valid = std::str::from_utf8(&bytes[..valid_end])
                            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                        if self.inner.push(valid)? == ScanControl::Stop {
                            self.stopped = true;
                            break;
                        }
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            if self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                                break;
                            }
                            bytes = &bytes[valid_end + invalid_len..];
                        }
                        None if final_chunk => {
                            if self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                            }
                            break;
                        }
                        None => {
                            let incomplete = &bytes[valid_end..];
                            if incomplete.len() >= self.pending.len() {
                                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                            }
                            self.pending[..incomplete.len()].copy_from_slice(incomplete);
                            self.pending_len = incomplete.len();
                            break;
                        }
                    }
                }
            }
        }
        Ok(if self.stopped {
            ScanControl::Stop
        } else {
            ScanControl::Continue
        })
    }
}

impl<S: TextBodyScanner> RawBodyScanner for LossyTextScanner<S> {
    type Output = S::Output;

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.feed_lossy(chunk, false)
    }

    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        if !self.stopped {
            self.feed_lossy(&[], true)?;
        }
        self.inner.finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyTextOperation {
    CharLen,
    IsEmpty,
    StartsWith(String),
    EndsWith(String),
    Contains(String),
    Equals(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyTextResult {
    CharLen(u64),
    Bool(bool),
}

pub(crate) enum TextOperationScanner {
    CharLen {
        count: u64,
    },
    StartsWith {
        pattern: Vec<u8>,
        matched: usize,
        failed: bool,
    },
    EndsWith {
        pattern: Vec<u8>,
        tail: VecDeque<u8>,
    },
    Contains {
        pattern: Vec<u8>,
        failure: Vec<usize>,
        matched: usize,
        found: bool,
    },
    Equals {
        pattern: Vec<u8>,
        matched: usize,
        failed: bool,
    },
}

impl TextOperationScanner {
    pub(crate) fn new(operation: BodyTextOperation) -> Self {
        match operation {
            BodyTextOperation::CharLen => Self::CharLen { count: 0 },
            BodyTextOperation::IsEmpty => Self::Equals {
                pattern: Vec::new(),
                matched: 0,
                failed: false,
            },
            BodyTextOperation::StartsWith(pattern) => Self::StartsWith {
                pattern: pattern.into_bytes(),
                matched: 0,
                failed: false,
            },
            BodyTextOperation::EndsWith(pattern) => Self::EndsWith {
                pattern: pattern.into_bytes(),
                tail: VecDeque::new(),
            },
            BodyTextOperation::Contains(pattern) => {
                let pattern = pattern.into_bytes();
                let failure = kmp_failure(&pattern);
                Self::Contains {
                    pattern,
                    failure,
                    matched: 0,
                    found: false,
                }
            }
            BodyTextOperation::Equals(pattern) => Self::Equals {
                pattern: pattern.into_bytes(),
                matched: 0,
                failed: false,
            },
        }
    }
}

pub(crate) fn empty_text_operation(
    operation: BodyTextOperation,
) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
    let value = match operation {
        BodyTextOperation::CharLen => BodyTextResult::CharLen(0),
        BodyTextOperation::IsEmpty => BodyTextResult::Bool(true),
        BodyTextOperation::StartsWith(pattern)
        | BodyTextOperation::EndsWith(pattern)
        | BodyTextOperation::Contains(pattern)
        | BodyTextOperation::Equals(pattern) => BodyTextResult::Bool(pattern.is_empty()),
    };
    Ok(BodyScanResult {
        value,
        bytes_scanned: 0,
    })
}

impl TextBodyScanner for TextOperationScanner {
    type Output = BodyTextResult;

    fn push(&mut self, chunk: &str) -> Result<ScanControl, BodyOperationError> {
        match self {
            Self::CharLen { count } => {
                let chunk_count = u64::try_from(chunk.chars().count())
                    .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                *count = count
                    .checked_add(chunk_count)
                    .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            }
            Self::StartsWith {
                pattern,
                matched,
                failed,
            } => {
                if pattern.is_empty() {
                    return Ok(ScanControl::Stop);
                }
                for &value in chunk.as_bytes() {
                    if *matched == pattern.len() {
                        return Ok(ScanControl::Stop);
                    }
                    if value != pattern[*matched] {
                        *failed = true;
                        return Ok(ScanControl::Stop);
                    }
                    *matched += 1;
                }
                if *matched == pattern.len() {
                    return Ok(ScanControl::Stop);
                }
            }
            Self::EndsWith { pattern, tail } => {
                if pattern.is_empty() {
                    return Ok(ScanControl::Stop);
                }
                for &value in chunk.as_bytes() {
                    if tail.len() == pattern.len() {
                        tail.pop_front();
                    }
                    tail.push_back(value);
                }
            }
            Self::Contains {
                pattern,
                failure,
                matched,
                found,
            } => {
                if pattern.is_empty() {
                    *found = true;
                    return Ok(ScanControl::Stop);
                }
                for &value in chunk.as_bytes() {
                    while *matched > 0 && pattern[*matched] != value {
                        *matched = failure[*matched - 1];
                    }
                    if pattern[*matched] == value {
                        *matched += 1;
                    }
                    if *matched == pattern.len() {
                        *found = true;
                        return Ok(ScanControl::Stop);
                    }
                }
            }
            Self::Equals {
                pattern,
                matched,
                failed,
            } => {
                for &value in chunk.as_bytes() {
                    if *matched >= pattern.len() || pattern[*matched] != value {
                        *failed = true;
                        return Ok(ScanControl::Stop);
                    }
                    *matched += 1;
                }
            }
        }
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(match self {
            Self::CharLen { count } => BodyTextResult::CharLen(count),
            Self::StartsWith {
                pattern,
                matched,
                failed,
            } => BodyTextResult::Bool(!failed && matched == pattern.len()),
            Self::EndsWith { pattern, tail } => {
                BodyTextResult::Bool(pattern.is_empty() || tail.iter().copied().eq(pattern))
            }
            Self::Contains { pattern, found, .. } => BodyTextResult::Bool(pattern.is_empty() || found),
            Self::Equals {
                pattern,
                matched,
                failed,
            } => BodyTextResult::Bool(!failed && matched == pattern.len()),
        })
    }
}

fn kmp_failure(pattern: &[u8]) -> Vec<usize> {
    let mut failure = vec![0; pattern.len()];
    let mut matched = 0usize;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = failure[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
            failure[index] = matched;
        }
    }
    failure
}

pub(crate) struct BytesScanner {
    bytes: Vec<u8>,
}

impl BytesScanner {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }
}

impl RawBodyScanner for BytesScanner {
    type Output = Bytes;

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.bytes.extend_from_slice(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(Bytes::from(self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_text_matches_lossy_decoding_at_every_split_and_output_limit() {
        let corpus: &[&[u8]] = &[
            b"",
            b"plain text",
            "aé€z".as_bytes(),
            &[b'a', 0xe2, 0x28, 0xa1, b'z'],
            &[0xc0, 0xaf, 0xff],
            &[0xed, 0xa0, 0x80],
            &[b'a', 0xe2, 0x82],
        ];
        for input in corpus {
            let expected = String::from_utf8_lossy(input);
            for first in 0..=input.len() {
                for second in first..=input.len() {
                    for limit in 0..=expected.len() + 1 {
                        let mut scanner = LossyTextScanner::new(BoundedTextCollector::new(limit));
                        for chunk in [&input[..first], &input[first..second], &input[second..]] {
                            if scanner.push(chunk).unwrap() == ScanControl::Stop {
                                break;
                            }
                        }
                        let result = scanner.finish().unwrap();
                        if expected.len() <= limit {
                            assert_eq!(result, BoundedTextResult::Complete(expected.to_string()));
                        } else {
                            assert_eq!(result, BoundedTextResult::OutputLimitExceeded);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bounded_text_stops_before_growth_and_discards_overflow_prefix() {
        let mut collector = BoundedTextCollector::new(6);
        assert_eq!(collector.push("abc").unwrap(), ScanControl::Continue);
        assert_eq!(collector.push("def").unwrap(), ScanControl::Continue);
        assert_eq!(collector.text, "abcdef");
        assert_eq!(collector.push("g").unwrap(), ScanControl::Stop);
        assert_eq!(collector.text.capacity(), 0);
        assert_eq!(collector.push("ignored").unwrap(), ScanControl::Stop);
        assert_eq!(collector.finish().unwrap(), BoundedTextResult::OutputLimitExceeded);

        let mut zero = BoundedTextCollector::new(0);
        assert_eq!(zero.push("").unwrap(), ScanControl::Continue);
        assert_eq!(zero.push("x").unwrap(), ScanControl::Stop);
        assert_eq!(zero.text.capacity(), 0);
    }

    #[test]
    fn bounded_text_capacity_grows_geometrically_within_limit() {
        for (capacity, required, limit, expected) in [
            (0, 3, 10, 3),
            (3, 4, 10, 6),
            (6, 7, 10, 10),
            (3, 9, 10, 9),
            (0, 0, 0, 0),
            (usize::MAX / 2 + 1, usize::MAX - 1, usize::MAX, usize::MAX),
            (usize::MAX - 1, usize::MAX, usize::MAX, usize::MAX),
        ] {
            let requested = BoundedTextCollector::growth_target(capacity, required, limit);
            assert_eq!(requested, expected);
            assert!(requested >= required);
            assert!(requested <= limit);
        }
        let limit = 3 * 1024 + 1;
        let mut collector = BoundedTextCollector::new(limit);
        let mut allocations = 0;
        let mut previous_capacity = 0;
        // Invalid raw bytes reach this collector as separate replacement
        // scalars. Growth must not reserve exactly one scalar each time.
        for _ in 0..1024 {
            assert_eq!(collector.push("\u{FFFD}").unwrap(), ScanControl::Continue);
            let capacity = collector.text.capacity();
            if capacity != previous_capacity {
                allocations += 1;
                assert!(capacity >= (previous_capacity * 2).min(limit));
                previous_capacity = capacity;
            }
        }
        assert!(allocations <= 12);
        assert_eq!(collector.text.len(), 3 * 1024);
        assert_eq!(collector.push("x").unwrap(), ScanControl::Continue);
        assert_eq!(collector.text.len(), limit);
        assert_eq!(collector.push("y").unwrap(), ScanControl::Stop);
    }

    #[test]
    fn bounded_text_reservation_failure_preserves_storage_error_class() {
        let mut collector = BoundedTextCollector::new(usize::MAX);
        assert_eq!(
            collector.reserve_output(usize::MAX),
            Err(BodyOperationError::Body(BodyReadError::Incomplete))
        );
        assert!(!collector.exceeded);
        assert_eq!(collector.text.capacity(), 0);
    }

    struct CollectText(String);

    impl TextBodyScanner for CollectText {
        type Output = String;

        fn push(&mut self, chunk: &str) -> Result<ScanControl, BodyOperationError> {
            self.0.push_str(chunk);
            Ok(ScanControl::Continue)
        }

        fn finish(self) -> Result<Self::Output, BodyOperationError> {
            Ok(self.0)
        }
    }

    #[test]
    fn lossy_utf8_matches_whole_document_semantics_at_every_two_way_split() {
        let input = [b'a', 0xe2, 0x82, 0xac, 0xf0, 0x90, 0x80, b'b', 0xff, 0xe2, 0x82];
        let expected = String::from_utf8_lossy(&input);

        for first in 0..=input.len() {
            for second in first..=input.len() {
                let mut scanner = LossyTextScanner::new(CollectText(String::new()));
                for chunk in [&input[..first], &input[first..second], &input[second..]] {
                    assert_eq!(scanner.push(chunk).unwrap(), ScanControl::Continue);
                    assert!(scanner.pending_len < scanner.pending.len());
                }
                assert_eq!(scanner.finish().unwrap(), expected);
            }
        }
    }

    #[test]
    fn lossy_utf8_carry_never_retains_the_complete_chunk() {
        let mut input = vec![b'x'; 64 * 1024];
        input.extend_from_slice(&[0xf0, 0x90]);
        let mut scanner = LossyTextScanner::new(CollectText(String::new()));

        assert_eq!(scanner.push(&input).unwrap(), ScanControl::Continue);
        assert_eq!(scanner.pending_len, 2);
        let output = scanner.finish().unwrap();
        assert_eq!(output, String::from_utf8_lossy(&input));
    }

    #[test]
    fn lossy_utf8_matches_whole_document_for_malformed_sequences_at_every_split() {
        let corpus: &[&[u8]] = &[
            &[0xe2, 0x28, 0xa1],
            &[0xf0, 0x90, 0x28, 0xbc],
            &[0xc0, 0xaf],
            &[0xed, 0xa0, 0x80],
            &[0xf4, 0x90, 0x80, 0x80],
            &[0x80, 0x81, 0xbf],
        ];
        for input in corpus {
            let expected = String::from_utf8_lossy(input);
            for first in 0..=input.len() {
                for second in first..=input.len() {
                    let mut scanner = LossyTextScanner::new(CollectText(String::new()));
                    for chunk in [&input[..first], &input[first..second], &input[second..]] {
                        assert_eq!(scanner.push(chunk).unwrap(), ScanControl::Continue);
                    }
                    assert_eq!(
                        scanner.finish().unwrap(),
                        expected,
                        "input={input:?}, splits={first}/{second}"
                    );
                }
            }
        }
    }

    fn scan_text_operation(operation: BodyTextOperation, chunks: &[&[u8]]) -> BodyTextResult {
        let mut scanner = LossyTextScanner::new(TextOperationScanner::new(operation));
        for chunk in chunks {
            if scanner.push(chunk).unwrap() == ScanControl::Stop {
                break;
            }
        }
        scanner.finish().unwrap()
    }

    #[test]
    fn byte_predicates_preserve_multibyte_semantics_across_every_raw_split() {
        let text = "a€b€";
        let bytes = text.as_bytes();
        for split in 0..=bytes.len() {
            let chunks = [&bytes[..split], &bytes[split..]];
            for (operation, expected) in [
                (BodyTextOperation::StartsWith("a€".to_string()), true),
                (BodyTextOperation::EndsWith("b€".to_string()), true),
                (BodyTextOperation::Contains("€b".to_string()), true),
                (BodyTextOperation::Equals(text.to_string()), true),
                (BodyTextOperation::Contains("€a".to_string()), false),
            ] {
                assert_eq!(scan_text_operation(operation, &chunks), BodyTextResult::Bool(expected));
            }
            assert_eq!(
                scan_text_operation(BodyTextOperation::CharLen, &chunks),
                BodyTextResult::CharLen(4)
            );
        }
    }

    #[test]
    fn predicate_patterns_retain_utf8_bytes_instead_of_expanded_chars() {
        let pattern = "€".repeat(1024);
        let expected_bytes = pattern.len();

        let TextOperationScanner::StartsWith { pattern, .. } =
            TextOperationScanner::new(BodyTextOperation::StartsWith(pattern))
        else {
            unreachable!()
        };
        assert_eq!(pattern.len(), expected_bytes);
        assert_eq!(pattern.len(), 3 * 1024);
    }

    #[test]
    fn known_empty_operations_resolve_without_building_scanner_state() {
        assert_eq!(
            empty_text_operation(BodyTextOperation::CharLen).unwrap(),
            BodyScanResult {
                value: BodyTextResult::CharLen(0),
                bytes_scanned: 0
            }
        );
        assert_eq!(
            empty_text_operation(BodyTextOperation::IsEmpty).unwrap().value,
            BodyTextResult::Bool(true)
        );
        for operation in [
            BodyTextOperation::StartsWith("large-pattern".repeat(1024)),
            BodyTextOperation::EndsWith("large-pattern".repeat(1024)),
            BodyTextOperation::Contains("large-pattern".repeat(1024)),
            BodyTextOperation::Equals("large-pattern".repeat(1024)),
        ] {
            assert_eq!(
                empty_text_operation(operation).unwrap().value,
                BodyTextResult::Bool(false)
            );
        }
        for operation in [
            BodyTextOperation::StartsWith(String::new()),
            BodyTextOperation::EndsWith(String::new()),
            BodyTextOperation::Contains(String::new()),
            BodyTextOperation::Equals(String::new()),
        ] {
            assert_eq!(
                empty_text_operation(operation).unwrap().value,
                BodyTextResult::Bool(true)
            );
        }
    }
}
