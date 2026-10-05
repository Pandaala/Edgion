//! Allocation-free retention algebra for ordered pure JSON rule matches.
//!
//! This foundation does not select values, evaluate predicates, publish variables,
//! or enable structured streaming. Callers must supply only surviving matches in
//! reference order. Duplicate subtree replacement belongs to a future publisher.

use std::fmt;

/// Checked arithmetic or incompatible accounting layouts prevented composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonMatchSummaryError {
    /// A count or byte charge cannot be represented by `usize`.
    Overflow,
    /// Summaries use different per-match entry descriptor charges.
    EntryBytesMismatch,
}

impl fmt::Display for JsonMatchSummaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Overflow => "JSON match summary arithmetic overflow",
            Self::EntryBytesMismatch => "JSON match summary entry charges differ",
        })
    }
}

impl std::error::Error for JsonMatchSummaryError {}

/// Logical TX retention after a pure member, including its unchanged base state.
///
/// These charges model retained lengths and entry descriptors, not allocator
/// capacities, peak allocation, or the memory occupied by the summary itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonMatchRetention {
    /// Maximum charge across the empty prefix and every applied match prefix.
    pub peak_bytes: usize,
    /// Charge after all summarized matches have been applied.
    pub final_bytes: usize,
}

/// Fixed-size, payload-free summary of an ordered sequence of pure matches.
///
/// For matched name plus transformed-value lengths `l_i` and descriptor charge
/// `e`, prefix `k` contributes `k*e + sum(l_i, i <= k) + l_k` bytes. The last
/// match is counted twice because TX retains both its collection entry and its
/// singular matched name/value. An earlier prefix can exceed the final charge.
///
/// Composition is associative when charges agree and arithmetic fits. It is not
/// commutative. The empty summary is an identity for its descriptor charge.
/// This summarizes retention only: selected counts, operator work, deferred
/// errors, captures, actions, and committed-state lifetime are outside its scope.
/// It cannot identify the first failing prefix or choose error precedence when
/// count and byte limits both fail; that requires additional ordered metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonMatchSummary {
    entry_bytes: usize,
    count: usize,
    total_lengths: usize,
    last_length: usize,
    peak_bytes: usize,
}

impl JsonMatchSummary {
    /// Create the empty identity using the caller's actual entry descriptor size.
    ///
    /// The descriptor size is fixed during composition: a single prefix maximum
    /// cannot be adjusted exactly afterward for a different descriptor size.
    pub const fn new(entry_bytes: usize) -> Self {
        Self {
            entry_bytes,
            count: 0,
            total_lengths: 0,
            last_length: 0,
            peak_bytes: 0,
        }
    }

    /// Number of matching values, including zero-length matches.
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Sum of matched name and transformed-value lengths, once per match.
    pub const fn total_lengths(&self) -> usize {
        self.total_lengths
    }

    /// Last matched name plus transformed-value length, or zero when empty.
    pub const fn last_length(&self) -> usize {
        self.last_length
    }

    /// Append one match. An error leaves the summary unchanged.
    pub fn append(&mut self, name_bytes: usize, transformed_value_bytes: usize) -> Result<(), JsonMatchSummaryError> {
        let length = add(name_bytes, transformed_value_bytes)?;
        let count = add(self.count, 1)?;
        let total_lengths = add(self.total_lengths, length)?;
        let final_bytes = add(add(multiply(count, self.entry_bytes)?, total_lengths)?, length)?;
        *self = Self {
            entry_bytes: self.entry_bytes,
            count,
            total_lengths,
            last_length: length,
            peak_bytes: self.peak_bytes.max(final_bytes),
        };
        Ok(())
    }

    /// Append a later ordered summary. An error leaves both operands unchanged.
    ///
    /// Even empty operands must have the same descriptor charge. No scalar
    /// payloads or sequence traversal are needed to compose the summaries.
    pub fn concat(&mut self, later: Self) -> Result<(), JsonMatchSummaryError> {
        if self.entry_bytes != later.entry_bytes {
            return Err(JsonMatchSummaryError::EntryBytesMismatch);
        }
        if later.count == 0 {
            return Ok(());
        }
        let count = add(self.count, later.count)?;
        let total_lengths = add(self.total_lengths, later.total_lengths)?;
        let offset = add(multiply(self.count, self.entry_bytes)?, self.total_lengths)?;
        let later_peak = add(offset, later.peak_bytes)?;
        *self = Self {
            entry_bytes: self.entry_bytes,
            count,
            total_lengths,
            last_length: later.last_length,
            peak_bytes: self.peak_bytes.max(later_peak),
        };
        Ok(())
    }

    /// Add an unchanged nonmatch TX base to peak and final logical retention.
    ///
    /// The base must exclude any inherited matched state cleared for this member.
    /// An empty summary returns the base for both charges; it does not instruct
    /// the caller to commit that cleared state for a nonmatching rule.
    pub fn retention(&self, base_bytes: usize) -> Result<JsonMatchRetention, JsonMatchSummaryError> {
        let final_bytes = add(
            add(multiply(self.count, self.entry_bytes)?, self.total_lengths)?,
            self.last_length,
        )?;
        Ok(JsonMatchRetention {
            peak_bytes: add(base_bytes, self.peak_bytes)?,
            final_bytes: add(base_bytes, final_bytes)?,
        })
    }
}

fn add(left: usize, right: usize) -> Result<usize, JsonMatchSummaryError> {
    left.checked_add(right).ok_or(JsonMatchSummaryError::Overflow)
}

fn multiply(left: usize, right: usize) -> Result<usize, JsonMatchSummaryError> {
    left.checked_mul(right).ok_or(JsonMatchSummaryError::Overflow)
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_summary_test.rs"]
mod tests;
