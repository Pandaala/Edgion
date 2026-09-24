use super::*;

fn slack_slice(byte: u8) -> Bytes {
    let mut bytes = BytesMut::with_capacity(64);
    bytes.extend_from_slice(&[byte]);
    bytes.freeze()
}

#[test]
fn repeated_normalization_near_handle_cap_preserves_prior_allocations() {
    let mut chain = SliceChain::default();
    let mut retained_pointers = Vec::new();

    for byte in 0..MAX_SLICE_HANDLES {
        chain.push(slack_slice(byte as u8), 1024).unwrap();
        chain.normalize_retained(1024).unwrap();

        let slices = chain.raw_slices();
        assert_eq!(slices.len(), byte + 1);
        for (slice, pointer) in slices.iter().zip(&retained_pointers) {
            assert_eq!(slice.as_ptr(), *pointer, "a previously normalized slice was recopied");
        }
        retained_pointers.push(slices.last().unwrap().as_ptr());
    }

    assert_eq!(chain.coalesce_count(), 0);
    assert_eq!(chain.copied_bytes(), 0);
    assert_eq!(chain.len(), MAX_SLICE_HANDLES);

    chain.push(Bytes::from_static(b"x"), 1024).unwrap();
    assert_eq!(chain.coalesce_count(), 1);
    chain.normalize_retained(1024).unwrap();
    assert_eq!(chain.handle_count(), 1);
    assert_eq!(chain.len(), MAX_SLICE_HANDLES + 1);
}

#[test]
fn failed_suffix_normalization_is_atomic_for_prefix_and_new_handle() {
    let mut chain = SliceChain::default();
    chain.push(slack_slice(b'a'), 8).unwrap();
    chain.normalize_retained(8).unwrap();
    chain.push(slack_slice(b'b'), 8).unwrap();

    let before = chain.raw_slices();
    assert!(chain.normalize_retained(1).is_err());
    let after = chain.raw_slices();

    assert_eq!(after, before);
    assert_eq!(after[0].as_ptr(), before[0].as_ptr());
    assert_eq!(after[1].as_ptr(), before[1].as_ptr());
    assert_eq!(chain.len(), 2);
}

#[test]
fn empty_and_already_normalized_chains_are_noops() {
    let mut empty = SliceChain::default();
    empty.normalize_retained(0).unwrap();

    let mut chain = SliceChain::default();
    chain.push(slack_slice(b'a'), 8).unwrap();
    chain.normalize_retained(8).unwrap();
    let pointer = chain.raw_slices()[0].as_ptr();
    let resident = chain.resident_capacity();
    chain.normalize_retained(resident).unwrap();
    assert_eq!(chain.raw_slices()[0].as_ptr(), pointer);
    assert!(chain.normalize_retained(resident - 1).is_err());
    assert_eq!(chain.raw_slices()[0].as_ptr(), pointer);
}
