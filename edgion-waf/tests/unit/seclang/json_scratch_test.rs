use super::*;

#[test]
fn inline_storage_never_borrows_heap_budget_credit() {
    let mut bytes = SmallVec::<[u8; 256]>::new();
    assert!(matches!(growth_target(&bytes, 256, 0), Ok(None)));
    bytes.extend_from_slice(&[0; 256]);
    assert!(matches!(growth_target(&bytes, 1, 256), Err(GrowthError::Retained)));
    assert!(matches!(growth_target(&bytes, 1, 257), Ok(Some(257))));
    bytes.try_reserve_exact(1).unwrap();
    assert_eq!(heap_capacity(&bytes), 257);
    bytes.push(0);
    assert!(matches!(growth_target(&bytes, 1, 1), Ok(Some(258))));
    bytes.truncate(1);
    assert_eq!(heap_capacity(&bytes), 257);
    assert!(matches!(growth_target(&bytes, 256, 0), Ok(None)));
}

#[test]
fn growth_arithmetic_is_checked_before_allocation() {
    let mut bytes = SmallVec::<[u8; 256]>::new();
    bytes.push(0);
    assert!(matches!(
        growth_target(&bytes, usize::MAX, 0),
        Err(GrowthError::Arithmetic)
    ));
    bytes.try_reserve_exact(256).unwrap();
    assert!(matches!(
        growth_target(&bytes, 257, usize::MAX),
        Err(GrowthError::Arithmetic)
    ));
}
