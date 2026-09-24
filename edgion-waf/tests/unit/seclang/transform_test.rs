use super::*;

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
