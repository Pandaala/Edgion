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
fn suffix_matches_lossy_text_across_all_three_chunk_splits() {
    for raw in [
        b"".as_slice(),
        b"0123456789abcdefsuffix",
        "a€b€".as_bytes(),
        b"a\xffb\xe2\x82",
    ] {
        let text = String::from_utf8_lossy(raw);
        for pattern in [
            "",
            "x",
            "ix",
            "suffix",
            "€",
            "b€",
            "ab",
            "longer-than-any-test-input-suffix",
        ] {
            let expected = BodyTextResult::Bool(text.ends_with(pattern));
            for first in 0..=raw.len() {
                for second in first..=raw.len() {
                    assert_eq!(
                        scan_text_operation(
                            BodyTextOperation::EndsWith(pattern.to_owned()),
                            &[&raw[..first], &raw[first..second], &raw[second..]],
                        ),
                        expected,
                        "pattern={pattern:?}, splits={first},{second}"
                    );
                }
            }
        }
    }

    // Short chunks rotate the existing tail before a long chunk replaces it.
    assert_eq!(
        scan_text_operation(
            BodyTextOperation::EndsWith("tail".to_owned()),
            &[b"abcd", b"e", b"f", b"g", b"large-final-tail"],
        ),
        BodyTextResult::Bool(true)
    );
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
