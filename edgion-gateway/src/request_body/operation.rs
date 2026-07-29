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

/// Adapts a text scanner to raw body chunks without materializing the body.
pub struct LossyTextScanner<S> {
    inner: S,
    pending: Vec<u8>,
    stopped: bool,
}

impl<S> LossyTextScanner<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            pending: Vec::with_capacity(4),
            stopped: false,
        }
    }

    fn feed_lossy(&mut self, bytes: &[u8], final_chunk: bool) -> Result<ScanControl, BodyOperationError>
    where
        S: TextBodyScanner,
    {
        if self.stopped {
            return Ok(ScanControl::Stop);
        }
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0usize;
        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(valid) => {
                    if !valid.is_empty() && self.inner.push(valid)? == ScanControl::Stop {
                        self.stopped = true;
                    }
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        let valid = std::str::from_utf8(&self.pending[consumed..valid_end])
                            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                        if self.inner.push(valid)? == ScanControl::Stop {
                            self.stopped = true;
                            consumed = valid_end;
                            break;
                        }
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            if self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                                consumed = valid_end + invalid_len;
                                break;
                            }
                            consumed = valid_end + invalid_len;
                        }
                        None if final_chunk => {
                            if self.inner.push("\u{FFFD}")? == ScanControl::Stop {
                                self.stopped = true;
                            }
                            consumed = self.pending.len();
                        }
                        None => {
                            consumed = valid_end;
                            break;
                        }
                    }
                }
            }
            if self.stopped {
                break;
            }
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
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
        pattern: Vec<char>,
        matched: usize,
        failed: bool,
    },
    EndsWith {
        pattern: Vec<char>,
        tail: VecDeque<char>,
    },
    Contains {
        pattern: Vec<char>,
        failure: Vec<usize>,
        matched: usize,
        found: bool,
    },
    Equals {
        pattern: Vec<char>,
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
                pattern: pattern.chars().collect(),
                matched: 0,
                failed: false,
            },
            BodyTextOperation::EndsWith(pattern) => Self::EndsWith {
                pattern: pattern.chars().collect(),
                tail: VecDeque::new(),
            },
            BodyTextOperation::Contains(pattern) => {
                let pattern: Vec<char> = pattern.chars().collect();
                let failure = kmp_failure(&pattern);
                Self::Contains {
                    pattern,
                    failure,
                    matched: 0,
                    found: false,
                }
            }
            BodyTextOperation::Equals(pattern) => Self::Equals {
                pattern: pattern.chars().collect(),
                matched: 0,
                failed: false,
            },
        }
    }
}

pub(crate) fn empty_text_operation(
    operation: BodyTextOperation,
) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
    Ok(BodyScanResult {
        value: TextOperationScanner::new(operation).finish()?,
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
                for value in chunk.chars() {
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
                for value in chunk.chars() {
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
                for value in chunk.chars() {
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
                for value in chunk.chars() {
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

fn kmp_failure(pattern: &[char]) -> Vec<usize> {
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
