use super::*;

#[test]
fn range_preserves_bytes_and_borrowed_storage_across_fragment_layouts() {
    let input = b"abcdefghijklmnop";
    for lengths in [vec![16], vec![1; 16], vec![3, 1, 7, 5], vec![0, 4, 0, 4, 8, 0]] {
        let mut chain = SliceChain::default();
        let mut segments = Vec::new();
        let mut offset = 0;
        for length in lengths {
            let bytes = Bytes::copy_from_slice(&input[offset..offset + length]);
            if length != 0 {
                segments.push((offset, offset + length, bytes.as_ptr()));
            }
            chain.push(bytes, 1024).unwrap();
            offset += length;
        }
        assert_eq!(offset, input.len());
        for start in 0..=input.len() {
            for end in start..=input.len() {
                let actual = chain.range(ByteRange::new(start, end)).unwrap();
                assert_eq!(actual.as_ref(), &input[start..end]);
                if start == end {
                    assert!(matches!(actual, Cow::Borrowed(_)));
                } else if let Some(&(left, _, pointer)) =
                    segments.iter().find(|&&(left, right, _)| left <= start && end <= right)
                {
                    assert!(matches!(actual, Cow::Borrowed(_)));
                    assert_eq!(actual.as_ptr(), pointer.wrapping_add(start - left));
                } else {
                    assert!(matches!(actual, Cow::Owned(_)));
                }
            }
        }
        assert!(chain.range(ByteRange::new(0, input.len() + 1)).is_none());
        assert!(chain.range(ByteRange::new(input.len() + 1, input.len() + 1)).is_none());
    }

    let mut chain = SliceChain::default();
    for _ in 0..=MAX_SLICE_HANDLES {
        chain.push(Bytes::from_static(b"x"), 1024).unwrap();
    }
    assert_eq!(chain.coalesce_count(), 1);
    let whole = chain.range(ByteRange::new(0, chain.len())).unwrap();
    for (start, end) in [(0, chain.len()), (1, 3), (255, 257), (257, 257)] {
        let actual = chain.range(ByteRange::new(start, end)).unwrap();
        assert!(matches!(actual, Cow::Borrowed(_)));
        assert_eq!(actual.as_ref(), &whole[start..end]);
        if start != end {
            assert_eq!(actual.as_ptr(), whole.as_ptr().wrapping_add(start));
        }
    }
    assert!(SliceChain::default().range(ByteRange::new(0, 0)).unwrap().is_empty());
}

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

#[test]
fn range_literal_comparison_borrows_sliced_and_coalesced_storage() {
    let input = b"message_delta";
    let mut chains = Vec::new();
    for split in 0..=input.len() {
        let mut chain = SliceChain::default();
        chain.push(Bytes::copy_from_slice(&input[..split]), 1024).unwrap();
        chain.push(Bytes::copy_from_slice(&input[split..]), 1024).unwrap();
        chains.push(chain);
    }
    let mut coalesced = SliceChain::default();
    for _ in 0..=MAX_SLICE_HANDLES {
        coalesced.push(Bytes::from_static(b"x"), 1024).unwrap();
    }
    assert_eq!(coalesced.coalesce_count(), 1);
    chains.push(coalesced);
    for chain in chains {
        for range in [
            ByteRange::new(0, chain.len()),
            ByteRange::new(1, chain.len() - 1),
            ByteRange::new(chain.len(), chain.len()),
        ] {
            let expected = chain.range(range).unwrap();
            let (matched, allocations) =
                crate::test_allocation_counter::count_allocations(|| chain.range_eq(range, expected.as_ref()));
            assert!(matched);
            assert_eq!(allocations, 0);
            if !expected.is_empty() {
                let mut different = expected.to_vec();
                *different.last_mut().unwrap() ^= 1;
                assert!(!chain.range_eq(range, &different));
            }
            assert!(!chain.range_eq(range, b"different length"));
        }
        assert!(!chain.range_eq(ByteRange::new(2, 1), b""));
        assert!(!chain.range_eq(ByteRange::new(chain.len() + 1, chain.len() + 1), b""));
    }
}
