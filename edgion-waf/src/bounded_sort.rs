//! Allocation-free sorting with caller-owned fallible comparison budgets.

use std::cmp::Ordering;

pub(crate) fn heapsort_by<T: Copy, E>(
    values: &mut [T],
    mut compare: impl FnMut(T, T) -> Result<Ordering, E>,
) -> Result<(), E> {
    fn sift<T: Copy, E>(
        values: &mut [T],
        mut root: usize,
        compare: &mut impl FnMut(T, T) -> Result<Ordering, E>,
    ) -> Result<(), E> {
        while let Some(left) = root
            .checked_mul(2)
            .and_then(|v| v.checked_add(1))
            .filter(|v| *v < values.len())
        {
            let mut child = left;
            if left + 1 < values.len() && compare(values[left], values[left + 1])? == Ordering::Less {
                child += 1;
            }
            if compare(values[root], values[child])? != Ordering::Less {
                break;
            }
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
    for root in (0..values.len() / 2).rev() {
        sift(values, root, &mut compare)?;
    }
    for end in (1..values.len()).rev() {
        values.swap(0, end);
        sift(&mut values[..end], 0, &mut compare)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/bounded_sort_test.rs"]
mod tests;
