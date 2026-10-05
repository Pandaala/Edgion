//! Shared capacity arithmetic for bounded inline JSON byte scratch.
use smallvec::{Array, SmallVec};

pub(super) enum GrowthError {
    Arithmetic,
    Retained,
}

pub(super) fn heap_capacity<A: Array<Item = u8>>(bytes: &SmallVec<A>) -> usize {
    if bytes.spilled() {
        bytes.capacity()
    } else {
        0
    }
}

/// Return a required heap target. Inline storage is not credit against the heap budget.
pub(super) fn growth_target<A: Array<Item = u8>>(
    bytes: &SmallVec<A>,
    additional: usize,
    remaining: usize,
) -> Result<Option<usize>, GrowthError> {
    let required = bytes.len().checked_add(additional).ok_or(GrowthError::Arithmetic)?;
    if required <= bytes.capacity() {
        return Ok(None);
    }
    let maximum = heap_capacity(bytes)
        .checked_add(remaining)
        .ok_or(GrowthError::Arithmetic)?;
    if required > maximum {
        return Err(GrowthError::Retained);
    }
    Ok(Some(required.max(bytes.capacity().saturating_mul(2)).min(maximum)))
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_scratch_test.rs"]
mod tests;
