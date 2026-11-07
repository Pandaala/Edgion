use super::*;

fn apply_transformations(
    input: &[u8],
    transformations: &[TransformationKind],
    limits: TransformLimits,
) -> Result<Vec<u8>, TransformError> {
    match apply_transformations_borrowed(input, transformations, limits)? {
        Cow::Owned(value) => Ok(value),
        Cow::Borrowed(value) => {
            let mut copy = Vec::new();
            copy.try_reserve_exact(value.len())
                .map_err(|_| TransformError::Allocation)?;
            copy.extend_from_slice(value);
            Ok(copy)
        }
    }
}

#[test]
fn borrowed_final_transform_preserves_identity_ownership_and_validation() {
    let input = vec![b'a'; 8192];
    let limits = TransformLimits {
        max_output_bytes: input.len(),
        max_work_bytes: input.len() * 8,
    };
    for transforms in [
        vec![],
        vec![TransformationKind::None],
        vec![TransformationKind::Lowercase],
        vec![TransformationKind::RemoveNulls],
        vec![TransformationKind::HtmlEntityDecode],
        vec![TransformationKind::UrlDecodeUni],
        vec![TransformationKind::Lowercase, TransformationKind::RemoveNulls],
    ] {
        let borrowed = apply_transformations_borrowed(&input, &transforms, limits).unwrap();
        assert!(matches!(borrowed, Cow::Borrowed(_)));
        assert_eq!(borrowed.as_ptr(), input.as_ptr());
        let owned = apply_transformations(&input, &transforms, limits).unwrap();
        assert_eq!(owned, input);
        assert_ne!(owned.as_ptr(), input.as_ptr());
    }
    for transforms in [
        vec![TransformationKind::Lowercase],
        vec![TransformationKind::Lowercase, TransformationKind::HexEncode],
    ] {
        let result = apply_transformations_borrowed(b"A", &transforms, limits).unwrap();
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(
            result.as_ref(),
            apply_transformations(b"A", &transforms, limits).unwrap()
        );
    }
    for work in [input.len() * 2 - 1, input.len() * 2] {
        let result = apply_transformations_borrowed(
            &input,
            &[TransformationKind::Lowercase],
            TransformLimits {
                max_work_bytes: work,
                ..limits
            },
        );
        if work == input.len() * 2 {
            assert!(matches!(result, Ok(Cow::Borrowed(_))));
        } else {
            assert_eq!(result.unwrap_err(), TransformError::WorkLimit { limit: work });
        }
    }
    assert_eq!(
        apply_transformations_borrowed(
            &input,
            &[],
            TransformLimits {
                max_output_bytes: input.len() - 1,
                ..limits
            },
        )
        .unwrap_err(),
        TransformError::OutputLimit { limit: input.len() - 1 },
    );
    assert_eq!(
        apply_transformations_borrowed(
            b"a",
            &[TransformationKind::None, TransformationKind::HexEncode],
            TransformLimits {
                max_output_bytes: 1,
                max_work_bytes: 16
            },
        )
        .unwrap_err(),
        TransformError::OutputLimit { limit: 1 },
    );
}

fn transform(kind: TransformationKind, input: &[u8]) -> Vec<u8> {
    apply_transformations(
        input,
        &[kind],
        TransformLimits {
            max_output_bytes: 4_096,
            max_work_bytes: 16_384,
        },
    )
    .unwrap()
}

#[test]
fn selected_crs_transformations_have_frozen_byte_behavior() {
    let cases: &[(TransformationKind, &[u8], &[u8])] = &[
        (TransformationKind::Base64Decode, b"PFRFU1Q+", b"<TEST>"),
        (TransformationKind::CmdLine, b"C^OMMAND /C DIR", b"command/c dir"),
        (TransformationKind::CompressWhitespace, b"a\t  b", b"a b"),
        (TransformationKind::CssDecode, b"a\\000062 c", b"abc"),
        (TransformationKind::EscapeSeqDecode, br"a\x62\143", b"abc"),
        (TransformationKind::HexEncode, b"A\0", b"4100"),
        (TransformationKind::HtmlEntityDecode, b"&lt;&#x41;&#66;", b"<AB"),
        (TransformationKind::JsDecode, br"a\x62\143", b"abc"),
        (TransformationKind::Length, b"four", b"4"),
        (TransformationKind::Lowercase, b"AbC", b"abc"),
        (TransformationKind::NormalizePath, b"/a/./b/../c/", b"/a/c/"),
        (TransformationKind::NormalizePathWin, br"C:\a\..\b", b"C:/b"),
        (TransformationKind::RemoveCommentsChar, b"a/*b*/c#d", b"abcd"),
        (TransformationKind::RemoveNulls, b"a\0b", b"ab"),
        (TransformationKind::RemoveWhitespace, b"a \tb\nc", b"abc"),
        (TransformationKind::ReplaceComments, b"a/*private*/b", b"a b"),
        (TransformationKind::UrlDecodeUni, b"%uFF1C+%41", b"< A"),
        (TransformationKind::Utf8ToUnicode, "é".as_bytes(), b"%u00e9"),
    ];
    for (kind, input, expected) in cases {
        assert_eq!(transform(*kind, input), *expected, "{kind:?}");
    }
    assert_eq!(transform(TransformationKind::Sha1, b"abc").len(), 20);
    assert_eq!(
        transform(
            TransformationKind::HtmlEntityDecode,
            b"&#00000000000000000000000000000000000000000000000123;"
        ),
        b"{"
    );
}

#[test]
fn output_and_work_limits_cover_exact_and_first_excess() {
    assert_eq!(
        apply_transformations(
            b"ab",
            &[TransformationKind::HexEncode],
            TransformLimits {
                max_output_bytes: 4,
                max_work_bytes: 6,
            }
        )
        .unwrap(),
        b"6162"
    );
    assert_eq!(
        apply_transformations(
            b"ab",
            &[TransformationKind::HexEncode],
            TransformLimits {
                max_output_bytes: 3,
                max_work_bytes: 6,
            }
        ),
        Err(TransformError::OutputLimit { limit: 3 })
    );
    assert_eq!(
        apply_transformations(
            b"ab",
            &[TransformationKind::HexEncode],
            TransformLimits {
                max_output_bytes: 4,
                max_work_bytes: 5,
            }
        ),
        Err(TransformError::WorkLimit { limit: 5 })
    );
}

#[test]
fn errors_never_render_transformed_content() {
    let error = apply_transformations(
        b"private-value",
        &[TransformationKind::HexEncode],
        TransformLimits {
            max_output_bytes: 1,
            max_work_bytes: usize::MAX,
        },
    )
    .unwrap_err();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("private"));
}

#[derive(Default)]
struct WalkProbe {
    cap: usize,
    stages: Vec<Vec<u8>>,
    pointers: Vec<usize>,
    changed: Vec<bool>,
    work: Vec<usize>,
    peaks: Vec<usize>,
    abort_stage: bool,
}
impl TransformObserver for WalkProbe {
    type Error = ();
    fn admit_scratch(&mut self, bytes: usize) -> Result<(), ()> {
        self.peaks.push(bytes);
        if bytes > self.cap {
            Err(())
        } else {
            Ok(())
        }
    }
    fn scratch_capacity(&mut self, bytes: usize) -> Result<(), ()> {
        if bytes > self.cap {
            Err(())
        } else {
            Ok(())
        }
    }
    fn checkpoint(&mut self, point: TransformCheckpoint) -> Result<(), ()> {
        match point {
            TransformCheckpoint::StepOutput { changed, .. } => self.changed.push(changed),
            TransformCheckpoint::StepComplete { work, .. } => self.work.push(work),
            _ => {}
        }
        Ok(())
    }
    fn stage(&mut self, _: usize, value: &[u8]) -> Result<(), ()> {
        if self.abort_stage {
            return Err(());
        }
        self.pointers.push(value.as_ptr() as usize);
        self.stages.push(value.to_vec());
        Ok(())
    }
}
fn walk_limits() -> TransformLimits {
    TransformLimits {
        max_output_bytes: 4096,
        max_work_bytes: 65536,
    }
}

#[test]
fn walker_borrows_identity_and_records_unchanged_steps() {
    let input = b"ABC";
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe {
        cap: 0,
        ..Default::default()
    };
    assert!(walk_transformations(
        input,
        &[TransformationKind::None],
        false,
        walk_limits(),
        &mut scratch,
        &mut probe
    )
    .is_ok());
    assert_eq!(probe.pointers, [input.as_ptr() as usize]);
    assert_eq!(scratch.retained_bytes().unwrap(), 0);
    let mut probe = WalkProbe {
        cap: 100,
        ..Default::default()
    };
    assert!(walk_transformations(
        input,
        &[
            TransformationKind::None,
            TransformationKind::Lowercase,
            TransformationKind::None
        ],
        true,
        walk_limits(),
        &mut scratch,
        &mut probe
    )
    .is_ok());
    assert_eq!(probe.stages, [b"ABC".to_vec(), b"abc".to_vec()]);
    assert_eq!(probe.changed, [false, true, false]);
    assert_eq!(probe.work, [6, 12, 18]);
}

#[test]
fn walker_rejects_growth_peak_before_mutating_existing_buffer() {
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe {
        cap: 100,
        ..Default::default()
    };
    assert!(walk_transformations(
        b"ABC",
        &[TransformationKind::Lowercase],
        false,
        walk_limits(),
        &mut scratch,
        &mut probe
    )
    .is_ok());
    let old_capacity = scratch.first.capacity();
    let old_pointer = scratch.first.as_ptr();
    let mut probe = WalkProbe {
        cap: old_capacity + 5,
        ..Default::default()
    };
    assert!(matches!(
        walk_transformations(
            b"ABCDEF",
            &[TransformationKind::Lowercase],
            false,
            walk_limits(),
            &mut scratch,
            &mut probe
        ),
        Err(TransformWalkError::Observer(()))
    ));
    assert_eq!(scratch.first.as_ptr(), old_pointer);
    assert_eq!(scratch.first, b"abc");
    assert_eq!(probe.peaks.last(), Some(&(old_capacity + 6)));
}

#[test]
fn walker_callback_abort_and_late_transform_error_are_distinct() {
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe {
        cap: 100,
        abort_stage: true,
        ..Default::default()
    };
    assert!(matches!(
        walk_transformations(
            b"ABC",
            &[TransformationKind::Lowercase],
            true,
            walk_limits(),
            &mut scratch,
            &mut probe
        ),
        Err(TransformWalkError::Observer(()))
    ));
    assert_eq!(scratch.retained_bytes().unwrap(), 0);
    let mut probe = WalkProbe {
        cap: 100,
        ..Default::default()
    };
    let limits = TransformLimits {
        max_output_bytes: 3,
        max_work_bytes: 100,
    };
    assert!(matches!(
        walk_transformations(
            b"ABC",
            &[TransformationKind::Lowercase, TransformationKind::HexEncode],
            true,
            limits,
            &mut scratch,
            &mut probe
        ),
        Err(TransformWalkError::Transformation(TransformError::OutputLimit {
            limit: 3
        }))
    ));
    assert_eq!(probe.stages, [b"ABC".to_vec(), b"abc".to_vec()]);
}

#[test]
fn normalize_path_stack_preserves_relative_absolute_and_windows_semantics() {
    for (input, expected) in [
        ("a/../", ""),
        ("./", ""),
        ("/../../", "/"),
        ("../../a/../", "../../"),
        ("a/b/../../c/", "c/"),
        ("a/../../b", "../b"),
        ("//a///./b/../", "/a/"),
        ("long-component/../other/../last", "last"),
    ] {
        assert_eq!(
            transform(TransformationKind::NormalizePath, input.as_bytes()),
            expected.as_bytes()
        );
    }
    assert_eq!(
        transform(TransformationKind::NormalizePathWin, b"\\a/b\\..\\c\\"),
        b"/a/c/"
    );
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe {
        cap: 6,
        ..Default::default()
    };
    assert!(walk_transformations(
        b"a/b/..",
        &[TransformationKind::NormalizePath],
        false,
        walk_limits(),
        &mut scratch,
        &mut probe
    )
    .is_ok());
    assert_eq!(scratch.retained_bytes().unwrap(), 6);
    assert_eq!(probe.stages, [b"a".to_vec()]);
}

#[test]
fn unchanged_lowercase_borrows_with_zero_scratch_and_preserves_work_limits() {
    let input = b"lowercase-123\0\xff";
    for multi_match in [false, true] {
        for work in [input.len() * 2 - 1, input.len() * 2] {
            let mut scratch = TransformScratch::default();
            let mut probe = WalkProbe::default();
            let result = walk_transformations(
                input,
                &[TransformationKind::Lowercase],
                multi_match,
                TransformLimits {
                    max_output_bytes: input.len(),
                    max_work_bytes: work,
                },
                &mut scratch,
                &mut probe,
            );
            if work == input.len() * 2 {
                assert!(matches!(result, Ok(CurrentTransform::Input)));
                assert_eq!(probe.stages, [input.to_vec()]);
                assert_eq!(probe.pointers, [input.as_ptr() as usize]);
                assert_eq!(probe.work, [work]);
            } else {
                assert!(matches!(
                    result,
                    Err(TransformWalkError::Transformation(TransformError::WorkLimit { .. }))
                ));
            }
            assert_eq!(probe.changed, [false]);
            assert_eq!(scratch.retained_bytes().unwrap(), 0);
        }
    }
}

#[test]
fn marker_free_decoding_borrows_and_keeps_work_and_output_errors() {
    let input = vec![b'a'; 4096];
    for kind in [
        TransformationKind::RemoveNulls,
        TransformationKind::HtmlEntityDecode,
        TransformationKind::UrlDecodeUni,
    ] {
        for multi_match in [false, true] {
            for work in [input.len() * 2 - 1, input.len() * 2] {
                let mut scratch = TransformScratch::default();
                let mut probe = WalkProbe::default();
                let result = walk_transformations(
                    &input,
                    &[kind],
                    multi_match,
                    TransformLimits {
                        max_output_bytes: input.len(),
                        max_work_bytes: work,
                    },
                    &mut scratch,
                    &mut probe,
                );
                if work == input.len() * 2 {
                    assert!(matches!(result, Ok(CurrentTransform::Input)));
                    assert_eq!(probe.pointers, [input.as_ptr() as usize]);
                    assert_eq!(probe.work, [work]);
                } else {
                    assert!(matches!(
                        result,
                        Err(TransformWalkError::Transformation(TransformError::WorkLimit { .. }))
                    ));
                }
                assert_eq!(scratch.retained_bytes().unwrap(), 0);
                assert_eq!(probe.changed, [false]);
            }
            let mut probe = WalkProbe {
                cap: usize::MAX,
                ..Default::default()
            };
            assert!(matches!(
                walk_transformations(
                    &input,
                    &[kind],
                    multi_match,
                    TransformLimits {
                        max_output_bytes: input.len() - 1,
                        max_work_bytes: usize::MAX,
                    },
                    &mut TransformScratch::default(),
                    &mut probe
                ),
                Err(TransformWalkError::Transformation(TransformError::OutputLimit {
                    limit: 4095
                }))
            ));
            assert!(probe.changed.is_empty());
        }
    }
}

#[test]
fn decoding_identity_matches_materialization_for_bytes_and_marker_sequences() {
    for kind in [
        TransformationKind::RemoveNulls,
        TransformationKind::HtmlEntityDecode,
        TransformationKind::UrlDecodeUni,
    ] {
        let mut inputs = (0_u8..=255).map(|byte| vec![byte]).collect::<Vec<_>>();
        inputs.extend([
            b"a%zz".to_vec(),
            b"a%20b".to_vec(),
            b"a+b".to_vec(),
            b"a&bogus;b".to_vec(),
            b"a&amp;b".to_vec(),
            b"a\0b".to_vec(),
        ]);
        for input in inputs {
            let mut expected = Vec::new();
            expected
                .try_reserve_exact(projected_output(kind, input.len()).unwrap().min(4096))
                .unwrap();
            apply_one(kind, &input, &mut expected, 4096).unwrap();
            for multi_match in [false, true] {
                let mut scratch = TransformScratch::default();
                let mut probe = WalkProbe {
                    cap: 4096,
                    ..Default::default()
                };
                let result =
                    walk_transformations(&input, &[kind], multi_match, walk_limits(), &mut scratch, &mut probe)
                        .unwrap_or_else(|_| panic!("expected bounded transformation success"));
                let output = match result {
                    CurrentTransform::Input => &input,
                    CurrentTransform::First => &scratch.first,
                    CurrentTransform::Second => &scratch.second,
                };
                assert_eq!(output, &expected);
                assert_eq!(probe.changed, [expected != input]);
                let stages = if multi_match && expected != input {
                    vec![input.clone(), expected.clone()]
                } else if multi_match {
                    vec![input.clone()]
                } else {
                    vec![expected.clone()]
                };
                assert_eq!(probe.stages, stages);
            }
        }
    }
}

#[test]
fn identity_decoders_keep_prior_buffer_and_validate_later_steps() {
    let recipe = [
        TransformationKind::Lowercase,
        TransformationKind::UrlDecodeUni,
        TransformationKind::HtmlEntityDecode,
        TransformationKind::RemoveNulls,
    ];
    for multi_match in [false, true] {
        let mut scratch = TransformScratch::default();
        let mut probe = WalkProbe {
            cap: 3,
            ..Default::default()
        };
        assert!(matches!(
            walk_transformations(b"ABC", &recipe, multi_match, walk_limits(), &mut scratch, &mut probe),
            Ok(CurrentTransform::First)
        ));
        assert_eq!(scratch.first, b"abc");
        assert_eq!(scratch.second.capacity(), 0);
        assert_eq!(probe.changed, [true, false, false, false]);
        let mut later = recipe.to_vec();
        later.push(TransformationKind::HexEncode);
        let mut probe = WalkProbe {
            cap: usize::MAX,
            ..Default::default()
        };
        assert!(matches!(
            walk_transformations(
                b"ABC",
                &later,
                multi_match,
                TransformLimits {
                    max_output_bytes: 3,
                    max_work_bytes: 65536
                },
                &mut scratch,
                &mut probe
            ),
            Err(TransformWalkError::Transformation(TransformError::OutputLimit {
                limit: 3
            }))
        ));
    }
}

#[test]
fn lowercase_identity_preserves_transformed_storage_and_later_validation() {
    for multi_match in [false, true] {
        let mut scratch = TransformScratch::default();
        let mut probe = WalkProbe {
            cap: 3,
            ..Default::default()
        };
        assert!(matches!(
            walk_transformations(
                b"ABC",
                &[TransformationKind::Lowercase, TransformationKind::Lowercase],
                multi_match,
                walk_limits(),
                &mut scratch,
                &mut probe,
            ),
            Ok(CurrentTransform::First)
        ));
        assert_eq!(scratch.retained_bytes().unwrap(), 3);
        assert_eq!(probe.changed, [true, false]);
        assert_eq!(probe.work, [6, 12]);
        assert_eq!(probe.stages.last().unwrap(), b"abc");
        assert_eq!(*probe.pointers.last().unwrap(), scratch.first.as_ptr() as usize);

        let mut probe = WalkProbe {
            cap: 100,
            ..Default::default()
        };
        assert!(matches!(
            walk_transformations(
                b"abc",
                &[TransformationKind::Lowercase, TransformationKind::HexEncode],
                multi_match,
                TransformLimits {
                    max_output_bytes: 3,
                    max_work_bytes: 100
                },
                &mut scratch,
                &mut probe,
            ),
            Err(TransformWalkError::Transformation(TransformError::OutputLimit {
                limit: 3
            }))
        ));
    }
}

#[test]
fn lowercase_borrowing_matches_ascii_mapping_for_every_byte() {
    for byte in 0u8..=u8::MAX {
        assert_eq!(
            transform(TransformationKind::Lowercase, &[byte]),
            [byte.to_ascii_lowercase()]
        );
    }
    assert_eq!(transform(TransformationKind::Lowercase, b""), b"");
}

#[test]
fn large_unchanged_lowercase_keeps_input_borrowed() {
    let input = vec![b'x'; 256 * 1024];
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe::default();
    assert!(matches!(
        walk_transformations(
            &input,
            &[TransformationKind::Lowercase],
            false,
            TransformLimits {
                max_output_bytes: input.len(),
                max_work_bytes: input.len() * 2,
            },
            &mut scratch,
            &mut probe,
        ),
        Ok(CurrentTransform::Input)
    ));
    assert_eq!(probe.pointers, [input.as_ptr() as usize]);
    assert_eq!(scratch.retained_bytes().unwrap(), 0);
    assert_eq!(probe.work, [input.len() * 2]);
}

#[test]
fn html_entity_delimiters_preserve_decoding_and_output_limits() {
    for (input, expected) in [
        (b"&".as_slice(), b"&".as_slice()),
        (b"&amp", b"&"),
        (b"&;", b"&;"),
        (b"&bogus;", b"&bogus;"),
        (b"&bogus&amp;", b"&bogus&"),
        (b"&&lt;", b"&<"),
        (b"&amp;&gt;", b"&>"),
        (b"&#65;&#x41;&#X41;", b"AAA"),
        (b"&#x;&amp;", b"&#x;&"),
        (b"prefix&unterminated", b"prefix&unterminated"),
        // References consumed without a terminating `;` (waf-03), alone and
        // followed by a tail, must keep the partial-output invariant.
        (b"&#58", b":"),
        (b"&#58x", b":x"),
        (b"&lt1", b"&lt1"),
        (b"&QUOT&Gt", b"\">"),
    ] {
        let mut output = Vec::new();
        html_entity_decode(input, &mut output, expected.len()).unwrap();
        assert_eq!(output, expected);
        for limit in 0..expected.len() {
            output.clear();
            assert_eq!(
                html_entity_decode(input, &mut output, limit),
                Err(TransformError::OutputLimit { limit }),
                "{}/{limit}",
                input.escape_ascii()
            );
            assert_eq!(output, expected[..limit], "{}/{limit}", input.escape_ascii());
        }
    }
}

/// waf-03: libmodsecurity v3 `html_entity_decode.cc` makes the terminating `;`
/// optional for named, decimal and hexadecimal references (consumed when
/// present), consumes the maximal digit or alphanumeric run, and compares the
/// whole name case-insensitively (`&ltest;` is not `&lt;`). Edgion keeps UTF-8
/// output for code points above 0xFF instead of the reference's one-byte cast.
#[test]
fn html_entity_references_follow_reference_semicolon_and_case_rules() {
    for (input, expected) in [
        (b"javascript&#58alert(1)".as_slice(), b"javascript:alert(1)".as_slice()),
        (b"&#x3c", b"<"),
        (b"&#X3Cz", b"<z"),
        (b"&#58;;", b":;"),
        (b"&#59;", b";"),
        (b"&LT;", b"<"),
        (b"&Lt", b"<"),
        (b"&lt", b"<"),
        (b"&amp", b"&"),
        (b"&AMP;x", b"&x"),
        (b"&ampx", b"&ampx"),
        (b"&ltest;", b"&ltest;"),
        (b"&lt1", b"&lt1"),
        (b"&QUOT&Gt&NbSp", b"\">\xc2\xa0"),
        (b"&Colon&TAB;&Sol", b":\t/"),
        (b"&#xZZ", b"&#xZZ"),
        (b"&#", b"&#"),
        (b"&#x", b"&#x"),
        (b"&#;", b"&#;"),
        (b"&#x13c;", b"\xc4\xbc"),
        (b"&#x110000;", b"\xef\xbf\xbd"),
        (b"&#99999999999;x", b"\xef\xbf\xbdx"),
        (b"a&lt&gt;b", b"a<>b"),
    ] {
        let mut output = Vec::new();
        html_entity_decode(input, &mut output, usize::MAX).unwrap();
        assert_eq!(output, expected, "{}", input.escape_ascii());
        let transformed = transform(TransformationKind::HtmlEntityDecode, input);
        assert_eq!(transformed, expected, "{}", input.escape_ascii());
    }
}

#[test]
fn comment_search_preserves_first_closer_and_output_limits() {
    for (input, expected) in [
        (b"".as_slice(), b"".as_slice()),
        (b"plain", b"plain"),
        (b"/*", b" "),
        (b"/*/", b" "),
        (b"/**/", b" "),
        (b"/***/", b" "),
        (b"/****/tail", b" tail"),
        (b"a/*outer/*inner*/tail*/b", b"a tail*/b"),
        (b"/**//**/", b"  "),
        (b"a/*closed*/b/*open", b"a b "),
        (b"*/tail", b"*/tail"),
    ] {
        let mut output = Vec::new();
        replace_comments(input, &mut output, expected.len()).unwrap();
        assert_eq!(output, expected);
        for limit in 0..expected.len() {
            output.clear();
            assert_eq!(
                replace_comments(input, &mut output, limit),
                Err(TransformError::OutputLimit { limit })
            );
            assert_eq!(output, expected[..limit]);
        }
    }
}

#[test]
fn css_hex_scan_preserves_digit_boundaries_and_output_limits() {
    for (input, expected) in [
        (b"".as_slice(), b"".as_slice()),
        (b"\\", b""),
        (b"\\\nX", b"X"),
        (b"\\\rX", b"\rX"),
        (br"\g", b"g"),
        (br"\0", b"\0"),
        (br"\41", b"A"),
        (br"\041", b"A"),
        (br"\0041", b"A"),
        (br"\00041", b"A"),
        (br"\000041", b"A"),
        (br"\0000412", b"A2"),
        (br"\1234567", b"V7"),
        (br"\123456g", b"Vg"),
        (br"\4a\4A", b"JJ"),
        (br"\ff21", b"A"),
        (br"\41 z", b"Az"),
        (br"\41  z", b"A z"),
        (b"\\000041\tX", b"AX"),
    ] {
        let mut output = Vec::new();
        css_decode(input, &mut output, expected.len()).unwrap();
        assert_eq!(output, expected);
        for limit in 0..expected.len() {
            output.clear();
            assert_eq!(
                css_decode(input, &mut output, limit),
                Err(TransformError::OutputLimit { limit })
            );
            assert_eq!(output, expected[..limit]);
        }
    }
}

#[test]
fn hex_escape_decoders_preserve_fallback_and_partial_output() {
    use TransformationKind::{EscapeSeqDecode, JsDecode, UrlDecodeUni};
    let cases: &[(TransformationKind, &[u8], &[u8])] = &[
        (EscapeSeqDecode, br"\x41\X4a", b"AJ"),
        (EscapeSeqDecode, br"\xZ1\x4Z", b"xZ1x4Z"),
        (EscapeSeqDecode, br"\x4", b"x4"),
        (EscapeSeqDecode, br"\x", b"x"),
        (EscapeSeqDecode, br"\u0041", b"u0041"),
        (EscapeSeqDecode, br"\777\477\n", b"\xff\x3f\n"),
        (EscapeSeqDecode, b"\\", b"\\"),
        (JsDecode, br"\x41\x4a\u0042\u00e9", b"AJB\xe9"),
        (JsDecode, br"\X41\U0041", b"X41U0041"),
        (JsDecode, br"\xZ1\x4Z", b"xZ1x4Z"),
        (JsDecode, br"\uG041\u0G41\u00G1\u004G", b"uG041u0G41u00G1u004G"),
        (JsDecode, br"\u004", b"u004"),
        (JsDecode, br"\u\x4\x", b"ux4x"),
        (JsDecode, br"\uff00\uff01\uff5e\uff5f", b"\0!~_"),
        (JsDecode, br"\777\477\n", b"?7'7\n"),
        (JsDecode, b"\\", b"\\"),
        (UrlDecodeUni, b"%41%4a%u0042%U00e9+", b"AJB\xe9 "),
        (UrlDecodeUni, b"%Z1%4Z", b"%Z1%4Z"),
        (UrlDecodeUni, b"%uG041%u0G41%u00G1%u004G", b"%uG041%u0G41%u00G1%u004G"),
        (UrlDecodeUni, b"%u004", b"%u004"),
        (UrlDecodeUni, b"%u%4%", b"%u%4%"),
        (UrlDecodeUni, b"%uff00%Uff01%uff5e%Uff5f", b"\0!~_"),
        (UrlDecodeUni, b"%x41%+41", b"%x41% 41"),
    ];
    for &(kind, input, decoded) in cases {
        // Keep a suffix to pin consumption, except for the dangling-backslash case.
        let tail = if input == b"\\" { b"".as_slice() } else { b"!" };
        let mut input_with_tail = input.to_vec();
        input_with_tail.extend_from_slice(tail);
        for prefix in [b"".as_slice(), b"P"] {
            let mut expected = prefix.to_vec();
            expected.extend_from_slice(decoded);
            expected.extend_from_slice(tail);
            for limit in prefix.len()..=expected.len() + 1 {
                let mut output = prefix.to_vec();
                let result = apply_one(kind, &input_with_tail, &mut output, limit);
                let expected_result = if limit < expected.len() {
                    Err(TransformError::OutputLimit { limit })
                } else {
                    Ok(())
                };
                assert_eq!(result, expected_result, "kind={kind:?}, input={input:?}, limit={limit}");
                assert_eq!(output, expected[..expected.len().min(limit)]);
            }
        }
    }
}

#[test]
fn octal_escape_decoders_preserve_width_and_partial_output() {
    use TransformationKind::{EscapeSeqDecode, JsDecode};
    fn check(kind: TransformationKind, input: &[u8], decoded: &[u8]) {
        for prefix in [b"".as_slice(), b"P"] {
            let mut expected = prefix.to_vec();
            expected.extend_from_slice(decoded);
            for limit in prefix.len()..=expected.len() + 1 {
                let mut output = prefix.to_vec();
                let result = apply_one(kind, input, &mut output, limit);
                assert_eq!(
                    result,
                    if limit < expected.len() {
                        Err(TransformError::OutputLimit { limit })
                    } else {
                        Ok(())
                    },
                    "kind={kind:?}, input={input:?}, limit={limit}"
                );
                assert_eq!(output, expected[..expected.len().min(limit)]);
            }
        }
    }
    for kind in [EscapeSeqDecode, JsDecode] {
        for first in 0_u8..8 {
            for second in 0_u8..8 {
                for third in 0_u8..8 {
                    let input = [b'\\', b'0' + first, b'0' + second, b'0' + third, b'7', b'!'];
                    if kind == JsDecode && first > 3 {
                        check(kind, &input, &[first * 8 + second, b'0' + third, b'7', b'!']);
                    } else {
                        let value = u16::from(first) * 64 + u16::from(second) * 8 + u16::from(third);
                        check(kind, &input, &[value as u8, b'7', b'!']);
                    }
                }
            }
        }
        for (input, expected) in [
            (br"\0".as_slice(), b"\0".as_slice()),
            (br"\7", b"\x07"),
            (br"\77", b"?"),
            (br"\78!", b"\x078!"),
            (br"\128!", b"\x0a8!"),
            (br"\0z", b"\0z"),
            (b"\\7\xff!", b"\x07\xff!"),
        ] {
            check(kind, input, expected);
        }
    }
}

const BACKSLASH_DECODERS: [TransformationKind; 3] = [
    TransformationKind::CssDecode,
    TransformationKind::EscapeSeqDecode,
    TransformationKind::JsDecode,
];

fn direct_backslash_decode(kind: TransformationKind, input: &[u8], limit: usize) -> Vec<u8> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(projected_output(kind, input.len()).unwrap().min(limit))
        .unwrap();
    match kind {
        TransformationKind::CssDecode => css_decode(input, &mut output, limit),
        TransformationKind::EscapeSeqDecode => escape_sequence_decode(input, &mut output, limit),
        TransformationKind::JsDecode => js_decode(input, &mut output, limit),
        _ => unreachable!("only backslash decoders are compared"),
    }
    .unwrap();
    output
}

#[test]
fn marker_free_css_js_escape_decoders_stay_borrowed() {
    let long = (0..4096)
        .map(|index| b"abc-xyz 0123&%+"[index % 15])
        .collect::<Vec<_>>();
    let exact = vec![b'q'; 2048];
    for kind in BACKSLASH_DECODERS {
        for (input, max_output_bytes) in [
            (b"abc".to_vec(), 4096),
            (long.clone(), 4096),
            (exact.clone(), exact.len()),
        ] {
            let limits = TransformLimits {
                max_output_bytes,
                max_work_bytes: 65536,
            };
            let result = apply_transformations_borrowed(&input, &[kind], limits).unwrap();
            assert!(
                matches!(result, Cow::Borrowed(_)),
                "{kind:?} must borrow marker-free input"
            );
            assert_eq!(result.as_ptr(), input.as_ptr());
            assert_eq!(result.as_ref(), input.as_slice());
            assert_eq!(result.as_ref(), direct_backslash_decode(kind, &input, max_output_bytes));
        }
    }
}

#[test]
fn marker_bearing_decoders_match_pre_change_bytes_and_work() {
    for kind in BACKSLASH_DECODERS {
        for input in [b"a\\".as_slice(), b"\\41", b"x\\u0041y"] {
            let expected = direct_backslash_decode(kind, input, 4096);
            let result = apply_transformations_borrowed(input, &[kind], walk_limits()).unwrap();
            assert!(matches!(result, Cow::Owned(_)), "{kind:?} must decode {input:?}");
            assert_eq!(result.as_ref(), expected.as_slice());
            for multi_match in [false, true] {
                let mut scratch = TransformScratch::default();
                let mut probe = WalkProbe {
                    cap: usize::MAX,
                    ..Default::default()
                };
                let location =
                    walk_transformations(input, &[kind], multi_match, walk_limits(), &mut scratch, &mut probe)
                        .unwrap_or_else(|_| panic!("expected bounded transformation success"));
                // multiMatch advances only past changed output; unchanged decoded
                // bytes leave the walk on its (equal) input.
                let output = match location {
                    CurrentTransform::Input if multi_match && expected == input => input,
                    CurrentTransform::First => &scratch.first[..],
                    _ => panic!("unexpected location for {kind:?} {input:?}"),
                };
                assert_eq!(output, expected.as_slice());
                assert_eq!(probe.work, [input.len() + expected.len()]);
                assert_eq!(probe.changed, [expected != input]);
                // The decode path admits a destination even when the bytes survive.
                assert!(scratch.retained_bytes().unwrap() > 0);
            }
        }
    }
    // A trailing lone backslash still takes the decode path: CSS drops it,
    // JavaScript and escape-sequence decoding keep it.
    assert_eq!(
        direct_backslash_decode(TransformationKind::CssDecode, b"a\\", 4096),
        b"a"
    );
    assert_eq!(
        direct_backslash_decode(TransformationKind::JsDecode, b"a\\", 4096),
        b"a\\"
    );
    assert_eq!(
        direct_backslash_decode(TransformationKind::EscapeSeqDecode, b"a\\", 4096),
        b"a\\"
    );
}

#[test]
fn marker_free_identity_steps_charge_same_work_as_decode() {
    let input = vec![b'z'; 1024];
    for kind in BACKSLASH_DECODERS {
        for multi_match in [false, true] {
            let mut identity_scratch = TransformScratch::default();
            let mut identity = WalkProbe::default();
            assert!(matches!(
                walk_transformations(
                    &input,
                    &[kind],
                    multi_match,
                    walk_limits(),
                    &mut identity_scratch,
                    &mut identity
                ),
                Ok(CurrentTransform::Input)
            ));
            // The identity step charges the pre-change decode work:
            // input.len() + output.len(), with output == input.
            assert_eq!(identity.work, [2 * input.len()]);
            assert_eq!(identity.changed, [false]);
            assert_eq!(identity_scratch.retained_bytes().unwrap(), 0);
            assert_eq!(identity.pointers, [input.as_ptr() as usize]);
            for work in [input.len() * 2 - 1, input.len() * 2] {
                let mut probe = WalkProbe::default();
                let result = walk_transformations(
                    &input,
                    &[kind],
                    multi_match,
                    TransformLimits {
                        max_output_bytes: input.len(),
                        max_work_bytes: work,
                    },
                    &mut TransformScratch::default(),
                    &mut probe,
                );
                if work == input.len() * 2 {
                    assert!(matches!(result, Ok(CurrentTransform::Input)));
                } else {
                    assert!(matches!(
                        result,
                        Err(TransformWalkError::Transformation(TransformError::WorkLimit { .. }))
                    ));
                }
            }
        }
    }
}

#[test]
fn multi_match_marker_free_decoder_does_not_stage() {
    let input = b"marker-free <script> value";
    let mut scratch = TransformScratch::default();
    let mut probe = WalkProbe::default();
    assert!(matches!(
        walk_transformations(
            input,
            &[TransformationKind::JsDecode, TransformationKind::CssDecode],
            true,
            walk_limits(),
            &mut scratch,
            &mut probe
        ),
        Ok(CurrentTransform::Input)
    ));
    // Exactly the initial stage(0, input): no ordinal advance after identity steps.
    assert_eq!(probe.stages, [input.to_vec()]);
    assert_eq!(probe.pointers, [input.as_ptr() as usize]);
    assert_eq!(probe.changed, [false, false]);
    assert_eq!(probe.work, [2 * input.len(), 4 * input.len()]);
    assert_eq!(scratch.retained_bytes().unwrap(), 0);
}
