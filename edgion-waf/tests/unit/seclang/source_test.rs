use super::{scan_source, LogicalLine, SourceError, SourceLimits, SourcePosition};

fn texts(document: &super::SourceDocument) -> Vec<&str> {
    document.lines().iter().map(LogicalLine::text).collect()
}

#[test]
fn scans_empty_comments_and_line_endings() {
    let empty = scan_source("empty", b"", SourceLimits::default()).unwrap();
    assert!(empty.lines().is_empty());
    assert_eq!(empty.physical_line_count(), 0);

    let document = scan_source(
        "rules.conf",
        b"\n \t# comment\r\n\t\r\n SecMarker START \nSecMarker END",
        SourceLimits::default(),
    )
    .unwrap();
    assert_eq!(texts(&document), ["SecMarker START", "SecMarker END"]);
    assert_eq!(document.physical_line_count(), 5);
    assert_eq!(document.source_id(), "rules.conf");
}

#[test]
fn joins_every_trailing_backslash_run_by_removing_one() {
    for run in 1..=4 {
        let slashes = "\\".repeat(run);
        let source = format!("SecAction {slashes}\nnext");
        let document = scan_source("runs", source.as_bytes(), SourceLimits::default()).unwrap();
        assert_eq!(
            document.lines()[0].text(),
            format!("SecAction {}next", "\\".repeat(run - 1))
        );
    }
}

#[test]
fn keeps_continuation_open_across_blank_and_comment_lines() {
    let document = scan_source(
        "continued",
        b"SecRule ARGS \\\n\n  # ignored\n  \"@rx test\"",
        SourceLimits::default(),
    )
    .unwrap();
    assert_eq!(texts(&document), ["SecRule ARGS \"@rx test\""]);
    assert_eq!(document.provenance_segment_count(), 2);
}

#[test]
fn maps_positions_across_continuation_segments() {
    let document = scan_source(
        "positions",
        b"  SecRule ARGS \\\n\t\"@rx test\"  ",
        SourceLimits::default(),
    )
    .unwrap();
    let line = &document.lines()[0];
    assert_eq!(line.text(), "SecRule ARGS \"@rx test\"");
    assert_eq!(line.provenance().len(), 2);
    assert_eq!(
        line.position_at(0),
        Some(SourcePosition {
            line: 1,
            column: 3,
            byte_offset: 2,
        })
    );
    assert_eq!(
        line.position_at("SecRule ARGS ".len()),
        Some(SourcePosition {
            line: 2,
            column: 2,
            byte_offset: 18,
        })
    );
    assert_eq!(
        line.position_at_end("SecRule ARGS ".len()),
        Some(SourcePosition {
            line: 1,
            column: 16,
            byte_offset: 15,
        })
    );
    assert_eq!(line.position_at_end(0), line.position_at(0));
    assert_eq!(line.position_at(line.text().len()), Some(line.span().end));
    assert_eq!(line.position_at_end(line.text().len()), Some(line.span().end));
    assert_eq!(line.position_at(line.text().len() + 1), None);
    assert_eq!(line.position_at_end(line.text().len() + 1), None);
}

#[test]
fn maps_join_boundaries_with_many_empty_segments_in_logarithmic_lookup() {
    let mut source = "\\\n".repeat(1_024);
    source.push_str("tail");
    let document = scan_source("many-segments", source.as_bytes(), SourceLimits::default()).unwrap();
    let line = &document.lines()[0];
    assert_eq!(line.text(), "tail");
    assert_eq!(line.provenance().len(), 1_025);
    assert_eq!(
        line.span().start,
        SourcePosition {
            line: 1,
            column: 1,
            byte_offset: 0,
        }
    );
    assert_eq!(
        line.position_at(0),
        Some(SourcePosition {
            line: 1_025,
            column: 1,
            byte_offset: 2_048,
        })
    );
}

#[test]
fn preserves_inline_hash_and_non_trim_whitespace() {
    let document = scan_source("hash", b"\x0bSecMarker A # retained\x0c", SourceLimits::default()).unwrap();
    assert_eq!(document.lines()[0].text(), "\x0bSecMarker A # retained\x0c");
}

#[test]
fn rejects_invalid_utf8_before_line_errors_without_echoing_content() {
    let error = scan_source("invalid", b"secret\xff\r", SourceLimits::default()).unwrap_err();
    assert_eq!(
        error,
        SourceError::InvalidUtf8 {
            position: SourcePosition {
                line: 1,
                column: 7,
                byte_offset: 6,
            },
        }
    );
    let rendered = error.to_string();
    assert!(!rendered.contains("secret"));
    assert!(!format!("{error:?}").contains("secret"));
}

#[test]
fn rejects_bare_carriage_returns() {
    let error = scan_source("bare-cr", b"a\rb", SourceLimits::default()).unwrap_err();
    assert!(matches!(
        error,
        SourceError::BareCarriageReturn {
            position: SourcePosition {
                line: 1,
                column: 2,
                byte_offset: 1,
            }
        }
    ));
}

#[test]
fn rejects_unfinished_continuation_after_ignored_lines() {
    let error = scan_source("unfinished", b"SecRule ARGS \\\n\n# ignored\n", SourceLimits::default()).unwrap_err();
    assert_eq!(
        error,
        SourceError::UnfinishedContinuation {
            position: SourcePosition {
                line: 4,
                column: 1,
                byte_offset: 26,
            },
        }
    );
    assert_eq!(
        scan_source("unfinished", b"a\\", SourceLimits::default()).unwrap_err(),
        SourceError::UnfinishedContinuation {
            position: SourcePosition {
                line: 1,
                column: 3,
                byte_offset: 2,
            },
        }
    );
}

#[test]
fn counts_final_newlines_without_an_implicit_line() {
    for (source, expected) in [(&b"\n"[..], 1), (&b"a\n"[..], 1), (&b"a\n\n"[..], 2), (&b"a"[..], 1)] {
        let document = scan_source("count", source, SourceLimits::default()).unwrap();
        assert_eq!(document.physical_line_count(), expected);
    }
}

#[test]
fn byte_columns_follow_multibyte_utf8_and_crlf() {
    let error = scan_source("columns", "é\r\nx\r".as_bytes(), SourceLimits::default()).unwrap_err();
    assert_eq!(
        error,
        SourceError::BareCarriageReturn {
            position: SourcePosition {
                line: 2,
                column: 2,
                byte_offset: 5,
            },
        }
    );
}

#[test]
fn accepts_exact_byte_limits_and_rejects_first_excess_byte() {
    let limits = SourceLimits {
        max_source_id_bytes: 2,
        max_source_bytes: 3,
        max_physical_line_bytes: 3,
        max_logical_line_bytes: 3,
        ..SourceLimits::default()
    };
    assert_eq!(scan_source("id", b"abc", limits).unwrap().lines()[0].text(), "abc");

    let error = scan_source("id", b"abcd", limits).unwrap_err();
    assert!(matches!(
        error,
        SourceError::SourceByteLimit {
            position: SourcePosition { byte_offset: 3, .. },
            ..
        }
    ));
    assert!(matches!(
        scan_source("ids", b"abc", limits),
        Err(SourceError::SourceIdLimit { actual: 3, limit: 2 })
    ));
}

#[test]
fn source_size_precedes_identifier_validation_and_identifier_is_owned() {
    let limits = SourceLimits {
        max_source_id_bytes: 1,
        max_source_bytes: 1,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("long-id", b"ab", limits),
        Err(SourceError::SourceByteLimit { .. })
    ));

    let mut source_id = String::from("owned");
    let document = scan_source(&source_id, b"a", SourceLimits::default()).unwrap();
    source_id.clear();
    assert_eq!(document.source_id(), "owned");
}

#[test]
fn enforces_physical_line_count_and_length() {
    let count_limits = SourceLimits {
        max_physical_lines: 1,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("count", b"a\nb", count_limits),
        Err(SourceError::PhysicalLineLimit {
            position: SourcePosition { line: 2, column: 1, .. },
            ..
        })
    ));

    let length_limits = SourceLimits {
        max_physical_line_bytes: 2,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("length", b"abc", length_limits),
        Err(SourceError::PhysicalLineByteLimit {
            position: SourcePosition { column: 3, .. },
            ..
        })
    ));
}

#[test]
fn enforces_logical_line_count_and_length() {
    let count_limits = SourceLimits {
        max_logical_lines: 1,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("count", b"a\nb", count_limits),
        Err(SourceError::LogicalLineLimit {
            position: SourcePosition { line: 2, .. },
            ..
        })
    ));

    let length_limits = SourceLimits {
        max_logical_line_bytes: 3,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("length", b"ab\\\ncd", length_limits),
        Err(SourceError::LogicalLineByteLimit {
            position: SourcePosition { line: 2, column: 2, .. },
            ..
        })
    ));
}

#[test]
fn enforces_provenance_and_owned_byte_budgets() {
    let provenance_limits = SourceLimits {
        max_provenance_segments: 1,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("segments", b"a\\\nb", provenance_limits),
        Err(SourceError::ProvenanceSegmentLimit {
            position: SourcePosition { line: 2, .. },
            ..
        })
    ));

    let owned_limits = SourceLimits {
        max_owned_bytes: 1,
        ..SourceLimits::default()
    };
    assert!(matches!(
        scan_source("id", b"", owned_limits),
        Err(SourceError::OwnedByteLimit { .. })
    ));
}

#[test]
fn bounds_metadata_for_many_tiny_lines() {
    let limits = SourceLimits {
        max_logical_lines: 3,
        max_provenance_segments: 3,
        ..SourceLimits::default()
    };
    let document = scan_source("tiny", b"a\nb\nc", limits).unwrap();
    assert_eq!(document.lines().len(), 3);
    assert_eq!(document.provenance_segment_count(), 3);
    assert!(document.charged_bytes() >= 3 * size_of::<LogicalLine>());
    assert!(matches!(
        scan_source("tiny", b"a\nb\nc\nd", limits),
        Err(SourceError::LogicalLineLimit { .. }) | Err(SourceError::ProvenanceSegmentLimit { .. })
    ));
}
