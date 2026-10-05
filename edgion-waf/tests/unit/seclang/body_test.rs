use super::*;

fn publication(bytes: &[u8], processor: BodyProcessorSelection, content_type: Option<&[u8]>) -> BodyPublication {
    build_publication(
        RequestBody::new(bytes, processor),
        content_type,
        0,
        BodyLimits::default(),
    )
    .unwrap()
}

fn values(publication: &BodyPublication, collection: CollectionKind) -> Vec<(Option<&[u8]>, &[u8])> {
    publication
        .entries
        .iter()
        .filter(|entry| entry.collection == collection)
        .map(|entry| (entry.name.as_deref(), entry.value.as_slice()))
        .collect()
}

#[test]
fn auto_and_urlencoded_preserve_duplicate_order_and_decode_tolerantly() {
    let decoded = publication(
        b"a=one&a=two%20words&flag=",
        BodyProcessorSelection::Auto,
        Some(b"application/x-www-form-urlencoded; charset=utf-8"),
    );
    assert_eq!(
        values(&decoded, CollectionKind::Args),
        [
            (Some(b"a".as_slice()), b"one".as_slice()),
            (Some(b"a".as_slice()), b"two words".as_slice()),
            (Some(b"flag".as_slice()), b"".as_slice()),
        ]
    );
    let malformed_escape = publication(b"bad=%Q0", BodyProcessorSelection::UrlEncoded, None);
    assert_eq!(
        values(&malformed_escape, CollectionKind::Args),
        [(Some(b"bad".as_slice()), b"%Q0".as_slice())]
    );
}

#[test]
fn empty_typed_bodies_publish_lengths_without_raw_document_values() {
    for content_type in [
        b"application/json".as_slice(),
        b"application/xml".as_slice(),
        b"multipart/form-data; boundary=empty".as_slice(),
    ] {
        let publication = publication(b"", BodyProcessorSelection::Auto, Some(content_type));
        assert!(values(&publication, CollectionKind::RequestBody).is_empty());
        assert_eq!(
            values(&publication, CollectionKind::ArgsCombinedSize),
            [(None, b"0".as_slice())]
        );
    }
}

#[test]
fn json_flattens_objects_arrays_and_scalar_types() {
    let publication = publication(
        br#"{"user":{"name":"alice"},"roles":["admin",null],"ok":true}"#,
        BodyProcessorSelection::Json,
        None,
    );
    assert_eq!(
        values(&publication, CollectionKind::Args),
        [
            (Some(b"json.ok".as_slice()), b"true".as_slice()),
            (Some(b"json.roles.0".as_slice()), b"admin".as_slice()),
            (Some(b"json.roles.1".as_slice()), b"".as_slice()),
            (Some(b"json.roles".as_slice()), b"2".as_slice()),
            (Some(b"json.user.name".as_slice()), b"alice".as_slice()),
        ]
    );
}

#[test]
fn multipart_publishes_fields_filenames_and_part_headers() {
    let body = concat!(
        "--edge\r\n",
        "Content-Disposition: form-data; name=\"title\"\r\n\r\n",
        "hello\r\n",
        "--edge\r\n",
        "Content-Disposition: form-data; name=\"upload\"; filename=\"a.txt\"\r\n",
        "Content-Type: text/plain\r\n\r\n",
        "content\r\n",
        "--edge--\r\n"
    );
    let publication = publication(
        body.as_bytes(),
        BodyProcessorSelection::Multipart,
        Some(b"multipart/form-data; boundary=edge"),
    );
    assert_eq!(
        values(&publication, CollectionKind::Args),
        [(Some(b"title".as_slice()), b"hello".as_slice())]
    );
    assert_eq!(
        values(&publication, CollectionKind::FilesNames),
        [(None, b"upload".as_slice())]
    );
    assert_eq!(
        values(&publication, CollectionKind::FilesCombinedSize),
        [(None, b"12".as_slice())]
    );
    assert_eq!(values(&publication, CollectionKind::MultipartPartHeaders).len(), 3);
}

#[test]
fn multipart_boundary_search_preserves_candidate_order_and_overlap() {
    fn old_search(input: &[u8], delimiter: &[u8], mut cursor: usize) -> Option<usize> {
        while cursor <= input.len() {
            let relative = find_bytes(input.get(cursor..)?, delimiter)?;
            let found = cursor + relative;
            let starts_line = found == 0 || input.get(found.saturating_sub(2)..found) == Some(b"\r\n");
            let after = found + delimiter.len();
            let ends_line = input.get(after..after + 2) == Some(b"\r\n") || input.get(after..after + 2) == Some(b"--");
            if starts_line && ends_line {
                return Some(found);
            }
            cursor = found + 1;
        }
        None
    }

    for boundary in [
        b"x".as_slice(),
        b"a\r\n--".as_slice(),
        b"---".as_slice(),
        b"\r".as_slice(),
        b"\n".as_slice(),
        b"\xffx".as_slice(),
        &[b'a'; 70],
        &[b'a'; 200],
    ] {
        let mut delimiter = b"--".to_vec();
        delimiter.extend_from_slice(boundary);
        let mut line_delimiter = b"\r\n".to_vec();
        line_delimiter.extend_from_slice(&delimiter);
        let searcher = if boundary.contains(&b'\r') || boundary.contains(&b'\n') {
            MultipartBoundarySearcher::Overlapping(multipart_prefix_table(&delimiter))
        } else {
            MultipartBoundarySearcher::Line(Finder::new(&line_delimiter))
        };
        let mut input = b"preamble\r\n".to_vec();
        input.extend_from_slice(&delimiter);
        input.extend_from_slice(b"X\r\n");
        input.extend_from_slice(&delimiter);
        input.extend_from_slice(b"\r\npart\r\n");
        input.extend_from_slice(&delimiter);
        input.extend_from_slice(b"--epilogue");
        for cursor in 0..=input.len() {
            assert_eq!(
                find_multipart_boundary(&input, &delimiter, &searcher, cursor),
                old_search(&input, &delimiter, cursor),
                "boundary={boundary:?}, cursor={cursor}"
            );
        }
    }

    let input = b"\r\n--a\r\n--a\r\n----";
    let delimiter = b"--a\r\n--";
    let prefix = multipart_prefix_table(delimiter);
    let searcher = MultipartBoundarySearcher::Overlapping(prefix);
    assert_eq!(find_multipart_boundary(input, delimiter, &searcher, 0), Some(7));

    let alphabet = [b'-', b'\r', b'\n', b'a', b'b', b'x'];
    let mut state = 1_u32;
    for _ in 0..256 {
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            alphabet[((state >> 16) as usize) % alphabet.len()]
        };
        let boundary: Vec<u8> = (0..(next() as usize % 8 + 1)).map(|_| next()).collect();
        let input: Vec<u8> = (0..96).map(|_| next()).collect();
        let mut delimiter = b"--".to_vec();
        delimiter.extend_from_slice(&boundary);
        let mut line = b"\r\n".to_vec();
        line.extend_from_slice(&delimiter);
        let searcher = if boundary.contains(&b'\r') || boundary.contains(&b'\n') {
            MultipartBoundarySearcher::Overlapping(multipart_prefix_table(&delimiter))
        } else {
            MultipartBoundarySearcher::Line(Finder::new(&line))
        };
        for cursor in 0..=input.len() {
            assert_eq!(
                find_multipart_boundary(&input, &delimiter, &searcher, cursor),
                old_search(&input, &delimiter, cursor),
                "boundary={boundary:?}, cursor={cursor}"
            );
        }
    }
}

#[test]
fn multipart_long_near_matches_preserve_fields_and_limits() {
    for boundary_len in [70, 200] {
        let boundary = vec![b'a'; boundary_len];
        let mut content_type = b"multipart/form-data; boundary=".to_vec();
        content_type.extend_from_slice(&boundary);
        let mut body = b"--".to_vec();
        body.extend_from_slice(&boundary);
        body.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\n");
        for _ in 0..1024 {
            body.extend_from_slice(b"\r\n--");
            body.extend_from_slice(&boundary[..boundary_len - 1]);
            body.push(b'b');
        }
        body.extend_from_slice(b"\r\n--");
        body.extend_from_slice(&boundary);
        body.extend_from_slice(b"--\r\n");
        let publication = publication(&body, BodyProcessorSelection::Multipart, Some(&content_type));
        assert_eq!(values(&publication, CollectionKind::Args).len(), 1);
        assert!(matches!(
            build_publication(
                RequestBody::new(&body, BodyProcessorSelection::Multipart),
                Some(&content_type),
                0,
                BodyLimits {
                    max_work_bytes: body.len(),
                    ..BodyLimits::default()
                },
            ),
            Err(BodyError::WorkLimit { .. })
        ));
    }
    let too_long = [b'a'; 201];
    let mut content_type = b"multipart/form-data; boundary=".to_vec();
    content_type.extend_from_slice(&too_long);
    assert!(matches!(
        build_publication(
            RequestBody::new(b"--x--\r\n", BodyProcessorSelection::Multipart),
            Some(&content_type),
            0,
            BodyLimits::default(),
        ),
        Err(BodyError::MultipartBoundary)
    ));
    assert!(matches!(
        build_publication(
            RequestBody::new(
                b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nv",
                BodyProcessorSelection::Multipart
            ),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits::default(),
        ),
        Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Multipart
        })
    ));
}

#[test]
fn multipart_disposition_keeps_semicolons_inside_quoted_parameters() {
    let disposition = b"form-data; name=\"prefix&amp;suffix\"; filename=\"safe.txt\"";
    assert_eq!(
        disposition_parameter(disposition, b"name"),
        Some(b"prefix&amp;suffix".as_slice())
    );
    assert_eq!(
        disposition_parameter(disposition, b"filename"),
        Some(b"safe.txt".as_slice())
    );
}

#[test]
fn xml_publishes_text_and_attributes_and_rejects_dtds() {
    let publication = publication(
        br#"<root id="42"><item>hello &amp; goodbye</item><![CDATA[raw]]></root>"#,
        BodyProcessorSelection::Xml,
        None,
    );
    let xml = publication
        .entries
        .iter()
        .filter(|entry| entry.collection == CollectionKind::Xml)
        .map(|entry| (entry.xml_attribute, entry.name.as_deref(), entry.value.as_slice()))
        .collect::<Vec<_>>();
    assert_eq!(
        xml,
        [
            (true, Some(b"id".as_slice()), b"42".as_slice()),
            (false, None, b"hello & goodbye".as_slice()),
            (false, None, b"raw".as_slice()),
        ]
    );
    assert!(matches!(
        build_publication(
            RequestBody::new(b"<!DOCTYPE x><x/>", BodyProcessorSelection::Xml),
            None,
            0,
            BodyLimits::default(),
        ),
        Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml
        })
    ));
}

#[test]
fn body_limits_accept_exact_and_reject_first_excess_without_content() {
    let exact = build_publication(
        RequestBody::new(b"abc", BodyProcessorSelection::Raw),
        None,
        0,
        BodyLimits {
            max_body_bytes: 3,
            ..BodyLimits::default()
        },
    )
    .unwrap();
    assert!(!exact.entries.is_empty());
    let Err(error) = build_publication(
        RequestBody::new(b"private", BodyProcessorSelection::Raw),
        None,
        0,
        BodyLimits {
            max_body_bytes: 6,
            ..BodyLimits::default()
        },
    ) else {
        panic!("first-excess body must fail");
    };
    assert_eq!(error, BodyError::ByteLimit { actual: 7, limit: 6 });
    assert!(!format!("{error:?} {error}").contains("private"));
}

#[test]
fn every_derived_body_limit_rejects_first_excess() {
    let raw = RequestBody::new(b"abc", BodyProcessorSelection::Raw);
    assert!(build_publication(
        raw,
        None,
        0,
        BodyLimits {
            max_entries: 4,
            max_decoded_bytes: 8,
            max_work_bytes: 3,
            ..BodyLimits::default()
        },
    )
    .is_ok());
    assert!(matches!(
        build_publication(
            raw,
            None,
            0,
            BodyLimits {
                max_entries: 3,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::EntryLimit { limit: 3 })
    ));
    assert!(matches!(
        build_publication(
            raw,
            None,
            0,
            BodyLimits {
                max_decoded_bytes: 7,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::DecodedByteLimit { limit: 7 })
    ));
    assert!(matches!(
        build_publication(
            raw,
            None,
            0,
            BodyLimits {
                max_work_bytes: 2,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::WorkLimit { limit: 2 })
    ));
    assert!(matches!(
        build_publication(
            RequestBody::new(b"name=value", BodyProcessorSelection::UrlEncoded),
            None,
            0,
            BodyLimits {
                max_name_bytes: 3,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::NameByteLimit { limit: 3 })
    ));
    assert!(matches!(
        build_publication(
            RequestBody::new(br#"{"a":{"b":1}}"#, BodyProcessorSelection::Json),
            None,
            0,
            BodyLimits {
                max_depth: 1,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::DepthLimit { limit: 1 })
    ));

    let multipart = concat!(
        "--x\r\n",
        "Content-Disposition: form-data; name=\"a\"\r\n",
        "X-Test: one\r\n\r\n",
        "1\r\n",
        "--x--\r\n"
    );
    assert!(matches!(
        build_publication(
            RequestBody::new(multipart.as_bytes(), BodyProcessorSelection::Multipart),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits {
                max_multipart_parts: 0,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::MultipartPartLimit { limit: 0 })
    ));
    assert!(matches!(
        build_publication(
            RequestBody::new(multipart.as_bytes(), BodyProcessorSelection::Multipart),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits {
                max_multipart_headers: 1,
                ..BodyLimits::default()
            },
        ),
        Err(BodyError::MultipartHeaderLimit { limit: 1 })
    ));
}

#[test]
fn malformed_xml_nesting_and_unknown_entities_fail_closed() {
    for body in [
        b"<root><child></root></child>".as_slice(),
        b"<root>&unknown;</root>".as_slice(),
        b"text<root/>".as_slice(),
    ] {
        assert!(matches!(
            build_publication(
                RequestBody::new(body, BodyProcessorSelection::Xml),
                None,
                0,
                BodyLimits::default(),
            ),
            Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml
            })
        ));
    }
    let numeric = publication(b"<root>&#65;&#x42;</root>", BodyProcessorSelection::Xml, None);
    assert!(values(&numeric, CollectionKind::Xml)
        .iter()
        .any(|(_, value)| *value == b"AB"));
}

#[test]
fn request_body_debug_is_content_free() {
    let body = RequestBody::new(b"private-secret", BodyProcessorSelection::Json);
    let rendered = format!("{body:?}");
    assert!(!rendered.contains("private"));
    assert!(rendered.contains("14"));
}

#[test]
fn json_duplicate_subtrees_replace_prior_values_before_sorted_publication() {
    let publication = publication(
        br#"{"z":"last","key":{"discarded":"attack"},"key":{"kept":"safe"},"a":[]}"#,
        BodyProcessorSelection::Json,
        None,
    );
    assert_eq!(
        values(&publication, CollectionKind::Args),
        [
            (Some(b"json.a".as_slice()), b"0".as_slice()),
            (Some(b"json.key.kept".as_slice()), b"safe".as_slice()),
            (Some(b"json.z".as_slice()), b"last".as_slice()),
        ]
    );
}

#[test]
fn json_reference_limits_apply_to_surviving_subtrees() {
    let overwritten = br#"{"a":{"long_name":["long-value","second"]},"a":0}"#;
    let surviving = br#"{"a":{"long_name":["long-value","second"]}}"#;
    let cases = [
        (
            BodyLimits {
                max_depth: 1,
                ..BodyLimits::default()
            },
            BodyError::DepthLimit { limit: 1 },
        ),
        (
            BodyLimits {
                max_name_bytes: 6,
                ..BodyLimits::default()
            },
            BodyError::NameByteLimit { limit: 6 },
        ),
        (
            BodyLimits {
                max_entries: 5,
                ..BodyLimits::default()
            },
            BodyError::EntryLimit { limit: 5 },
        ),
        (
            BodyLimits {
                max_decoded_bytes: 27,
                ..BodyLimits::default()
            },
            BodyError::DecodedByteLimit { limit: 27 },
        ),
    ];
    for (limits, expected_error) in cases {
        let result = build_publication(
            RequestBody::new(overwritten, BodyProcessorSelection::Json),
            None,
            0,
            limits,
        )
        .unwrap();
        assert_eq!(
            values(&result, CollectionKind::Args),
            [(Some(b"json.a".as_slice()), b"0".as_slice())]
        );
        assert_eq!(
            build_publication(
                RequestBody::new(surviving, BodyProcessorSelection::Json),
                None,
                0,
                limits,
            )
            .err(),
            Some(expected_error)
        );
    }
    // Source bytes are always charged, but only surviving node paths add work.
    let work_limit = overwritten.len() + b"json".len() + b"json.a".len();
    let result = build_publication(
        RequestBody::new(overwritten, BodyProcessorSelection::Json),
        None,
        0,
        BodyLimits {
            max_work_bytes: work_limit,
            ..BodyLimits::default()
        },
    )
    .unwrap();
    assert_eq!(result.entries.len(), 5);
    assert_eq!(
        build_publication(
            RequestBody::new(overwritten, BodyProcessorSelection::Json),
            None,
            0,
            BodyLimits {
                max_work_bytes: work_limit - 1,
                ..BodyLimits::default()
            },
        )
        .err(),
        Some(BodyError::WorkLimit { limit: work_limit - 1 })
    );
}

#[test]
fn json_external_path_collisions_preserve_distinct_entries_and_reference_order() {
    let result = publication(
        br#"{"a.b":"literal","a":{"b":"nested"},"":"empty","arr":[{},[],null]}"#,
        BodyProcessorSelection::Json,
        None,
    );
    assert_eq!(
        values(&result, CollectionKind::Args),
        [
            (Some(b"json.".as_slice()), b"empty".as_slice()),
            (Some(b"json.a.b".as_slice()), b"nested".as_slice()),
            (Some(b"json.a.b".as_slice()), b"literal".as_slice()),
            (Some(b"json.arr.1".as_slice()), b"0".as_slice()),
            (Some(b"json.arr.2".as_slice()), b"".as_slice()),
            (Some(b"json.arr".as_slice()), b"3".as_slice()),
        ]
    );
}

#[test]
fn json_numbers_publish_reference_normalization_including_scalar_roots() {
    for (input, expected) in [
        (b"1e2".as_slice(), b"100.0".as_slice()),
        (b"-0".as_slice(), b"-0.0".as_slice()),
        (b"1.500".as_slice(), b"1.5".as_slice()),
        (b"18446744073709551615".as_slice(), b"18446744073709551615".as_slice()),
    ] {
        let result = publication(input, BodyProcessorSelection::Json, None);
        assert_eq!(
            values(&result, CollectionKind::Args),
            [(Some(b"json".as_slice()), expected)]
        );
    }
}

#[test]
fn json_duplicate_replacement_cannot_hide_invalid_earlier_values() {
    for body in [
        br#"{"a":1e400,"a":0}"#.as_slice(),
        br#"{"a":[1,],"a":0}"#.as_slice(),
        b"{\"a\":\"\xff\",\"a\":0}".as_slice(),
    ] {
        assert_eq!(
            build_publication(
                RequestBody::new(body, BodyProcessorSelection::Json),
                None,
                0,
                BodyLimits::default(),
            )
            .err(),
            Some(BodyError::Malformed {
                processor: BodyProcessorSelection::Json
            }),
        );
    }
}

struct JsonDocumentFixture {
    document: crate::seclang::JsonDocument<usize>,
    scalars: Vec<(String, Vec<u8>)>,
}

fn document_fixture(input: &[u8], split: usize) -> JsonDocumentFixture {
    use crate::seclang::{JsonDocument, JsonDocumentLimits, JsonStreamInput, JsonStreamLimits};

    let mut document = JsonDocument::new(
        JsonStreamLimits::default(),
        JsonDocumentLimits {
            max_retained_bytes: 16 * 1024 * 1024,
            max_key_bytes: 2 * 1024 * 1024,
            max_members: 65_536,
            max_events: 131_072,
            max_path_bytes: 16 * 1024,
            max_depth: 64,
            max_work: 128 * 1024 * 1024,
        },
    );
    // Test-only payload table lets a fixed-size result identify the exact
    // borrowed callback value after duplicate removal and reference sorting.
    let mut scalars = Vec::new();
    let mut sink = |path: &str, value: &[u8]| {
        let id = scalars.len();
        scalars.push((path.to_owned(), value.to_vec()));
        Ok(id)
    };
    document
        .feed(JsonStreamInput::Data(&input[..split]), &mut sink)
        .unwrap();
    assert!(document.events().is_none());
    document
        .feed(JsonStreamInput::Data(&input[split..]), &mut sink)
        .unwrap();
    assert!(document.events().is_none());
    document.feed(JsonStreamInput::Complete, &mut sink).unwrap();
    JsonDocumentFixture { document, scalars }
}

fn replay_document_publication(
    input: &[u8],
    fixture: &JsonDocumentFixture,
    limits: BodyLimits,
) -> Result<BodyPublication, BodyError> {
    use crate::seclang::JsonDocumentEvent;

    if input.len() > limits.max_body_bytes {
        return Err(BodyError::ByteLimit {
            actual: input.len(),
            limit: limits.max_body_bytes,
        });
    }
    let mut builder = BodyBuilder::new(limits);
    builder.charge_work(input.len())?;
    builder.push(
        CollectionKind::RequestBodyLength,
        None,
        input.len().to_string().as_bytes(),
        false,
    )?;
    builder.push(CollectionKind::ReqbodyProcessor, None, b"JSON", false)?;
    let mut combined_size = 17_usize;
    for event in fixture.document.events().unwrap() {
        match event {
            JsonDocumentEvent::ExtendPath { path_bytes } => {
                if path_bytes > limits.max_name_bytes {
                    return Err(BodyError::NameByteLimit {
                        limit: limits.max_name_bytes,
                    });
                }
            }
            JsonDocumentEvent::Visit { path_bytes, depth } => {
                if depth > limits.max_depth {
                    return Err(BodyError::DepthLimit {
                        limit: limits.max_depth,
                    });
                }
                builder.charge_work(path_bytes)?;
            }
            JsonDocumentEvent::Scalar {
                path_bytes,
                value_bytes,
                result,
            } => {
                let (path, value) = &fixture.scalars[result];
                assert_eq!(path_bytes, path.len());
                assert_eq!(value_bytes, value.len());
                builder.push(CollectionKind::Args, Some(path.as_bytes()), value, false)?;
                builder.push(CollectionKind::ArgsNames, Some(path.as_bytes()), path.as_bytes(), false)?;
                combined_size += path.len() + value.len();
            }
        }
    }
    builder.push(
        CollectionKind::ArgsCombinedSize,
        None,
        combined_size.to_string().as_bytes(),
        false,
    )?;
    builder.finish()
}

fn assert_document_publication(input: &[u8], fixture: &JsonDocumentFixture, limits: BodyLimits) {
    let actual = replay_document_publication(input, fixture, limits);
    let expected = build_publication(RequestBody::new(input, BodyProcessorSelection::Json), None, 17, limits);
    let validated = validate_json_publication(&fixture.document, 17, limits, false);
    match (actual, expected) {
        (Ok(actual), Ok(expected)) => {
            let summary = validated.unwrap();
            assert_eq!(summary.entries, expected.entries.len());
            assert_eq!(summary.retained_bytes, expected.retained_bytes);
            let decoded: usize = expected
                .entries
                .iter()
                .map(|entry| entry.name.as_ref().map_or(0, |name| name.len()) + entry.value.len())
                .sum();
            let args: usize = expected
                .entries
                .iter()
                .filter(|entry| entry.collection == CollectionKind::Args)
                .map(|entry| entry.name.as_ref().map_or(0, |name| name.len()) + entry.value.len())
                .sum();
            assert_eq!(summary.decoded_bytes, decoded);
            assert_eq!(summary.body_args_combined_size, args);
            assert_eq!(summary.args_combined_size, 17 + args);
            assert_eq!(actual.retained_bytes, expected.retained_bytes);
            let view = |publication: BodyPublication| {
                publication
                    .entries
                    .into_iter()
                    .map(|entry| (entry.collection, entry.name, entry.value))
                    .collect::<Vec<_>>()
            };
            assert_eq!(view(actual), view(expected));
        }
        (Err(actual), Err(expected)) => {
            assert_eq!(actual, expected);
            assert_eq!(validated, Err(JsonPublicationError::Body(expected)));
        }
        _ => panic!("document replay and complete publication disagree on success"),
    }
}

#[test]
fn incremental_json_document_matches_complete_publication_at_every_split() {
    for input in [
        b"{}".as_slice(),
        b"[]",
        b"null",
        b"\"root\"",
        b"1e2",
        b"-0",
        br#"{"a":{}}"#,
        br#"{"a":[]}"#,
        br#"[[],{},null]"#,
        br#"{"z":"last","a":"first","m":true}"#,
        br#"{"a.b":"literal","a":{"b":"nested"},"":"empty"}"#,
        br#"{"a":0,"\u0061":1}"#,
        br#"{"a":{"discard":[1,2]},"a":"kept"}"#,
        br#"{"a":"removed","a":{}}"#,
        br#"[0,1,2,3,4,5,6,7,8,9,10,11]"#,
        br#"{"n":18446744073709551615,"f":1.500,"b":false}"#,
        br#"{"a":{"x":1,"x":2},"b":{"x":3},"a":{"x":4}}"#,
        r#"{"é":1,"a":0,"\u00e9":2,"ñ":3}"#.as_bytes(),
    ] {
        for split in 0..=input.len() {
            let fixture = document_fixture(input, split);
            assert_document_publication(input, &fixture, BodyLimits::default());
        }
    }
}

#[test]
fn incremental_json_document_preserves_competing_publication_limit_order() {
    for input in [
        b"{}".as_slice(),
        b"[]",
        b"null",
        br#"{"a":{}}"#,
        br#"{"a":[]}"#,
        br#"{"z":[],"a":{"b":"long"}}"#,
        br#"{"a":{"discard":[1,2]},"a":0}"#,
        br#"[[],{},null]"#,
    ] {
        let fixture = document_fixture(input, input.len() / 2);
        // Cross limits deliberately: error precedence matters when more than
        // one bound would fail, including metadata prelude and empty nodes.
        for max_entries in [0, 1, 2, 3, 4, 5, 8, 12] {
            for max_name_bytes in [0, 3, 4, 6, 8] {
                for max_depth in [0, 1, 2] {
                    for max_decoded_bytes in [0, 5, 12, 24, 60] {
                        for max_work_bytes in [0, input.len(), input.len() + 4, input.len() + 20, 128] {
                            assert_document_publication(
                                input,
                                &fixture,
                                BodyLimits {
                                    max_entries,
                                    max_name_bytes,
                                    max_depth,
                                    max_decoded_bytes,
                                    max_work_bytes,
                                    ..BodyLimits::default()
                                },
                            );
                        }
                    }
                }
            }
        }
        // Dense work thresholds also detect off-by-one visit lengths on
        // empty containers, whose paths never appear in scalar callbacks.
        for max_work_bytes in input.len()..=input.len() + 128 {
            assert_document_publication(
                input,
                &fixture,
                BodyLimits {
                    max_work_bytes,
                    ..BodyLimits::default()
                },
            );
        }
        for max_body_bytes in [input.len() - 1, input.len()] {
            assert_document_publication(
                input,
                &fixture,
                BodyLimits {
                    max_body_bytes,
                    ..BodyLimits::default()
                },
            );
        }
    }
}

#[test]
fn json_reference_prelude_limits_precede_syntax_but_traversal_limits_do_not() {
    let malformed = br#"{"long_name":0} trailing"#;
    for (limits, error) in [
        (
            BodyLimits {
                max_entries: 0,
                ..BodyLimits::default()
            },
            BodyError::EntryLimit { limit: 0 },
        ),
        (
            BodyLimits {
                max_work_bytes: 0,
                ..BodyLimits::default()
            },
            BodyError::WorkLimit { limit: 0 },
        ),
        (
            BodyLimits {
                max_name_bytes: 0,
                max_depth: 0,
                ..BodyLimits::default()
            },
            BodyError::Malformed {
                processor: BodyProcessorSelection::Json,
            },
        ),
    ] {
        assert_eq!(
            build_publication(
                RequestBody::new(malformed, BodyProcessorSelection::Json),
                None,
                0,
                limits,
            )
            .err(),
            Some(error)
        );
    }
}

#[test]
fn incremental_json_document_matches_generated_nested_duplicate_objects() {
    fn next(state: &mut u64) -> usize {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*state >> 32) as usize
    }
    fn value(state: &mut u64, depth: usize) -> String {
        match next(state) % if depth == 0 { 4 } else { 6 } {
            0 => "null".to_owned(),
            1 => next(state).is_multiple_of(2).to_string(),
            2 => format!("{}e-1", next(state) % 1000),
            3 => serde_json::to_string(["text", "", "quote\"slash\\", "é"][next(state) % 4]).unwrap(),
            4 => {
                let mut members = Vec::new();
                for _ in 0..next(state) % 6 {
                    let key = ["a", "b", "a", "a.b", ""][next(state) % 5];
                    members.push(format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        value(state, depth - 1)
                    ));
                }
                format!("{{{}}}", members.join(","))
            }
            _ => {
                let mut children = Vec::new();
                for _ in 0..next(state) % 5 {
                    children.push(value(state, depth - 1));
                }
                format!("[{}]", children.join(","))
            }
        }
    }
    for seed in 1..=128 {
        let mut state = seed;
        let input = value(&mut state, 4);
        for split in [0, input.len() / 3, input.len()] {
            let fixture = document_fixture(input.as_bytes(), split);
            assert_document_publication(input.as_bytes(), &fixture, BodyLimits::default());
            for max_entries in [3, 7, 15] {
                assert_document_publication(
                    input.as_bytes(),
                    &fixture,
                    BodyLimits {
                        max_entries,
                        ..BodyLimits::default()
                    },
                );
            }
        }
    }
}

#[test]
fn publication_accounting_keeps_literal_cross_format_boundaries() {
    use BodyProcessorSelection::{Json, Multipart, Raw, UrlEncoded, Xml};
    // Literal counts were derived from the pre-extraction implementation.
    // Only the descriptor size remains platform-dependent.
    let multipart = b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nb\r\n--x--\r\n";
    let cases = [
        (Raw, b"abc".as_slice(), None, false, None, 4, 9, 3, b"17".as_slice()),
        (Raw, b"abc", None, false, Some(false), 3, 6, 3, b"17"),
        (Raw, b"", None, false, None, 4, 6, 0, b"17"),
        (Json, b"", None, false, None, 3, 7, 0, b"17"),
        (Json, b"", None, false, Some(true), 4, 7, 0, b"17"),
        (UrlEncoded, b"a=b", None, false, None, 6, 20, 6, b"19"),
        (Json, br#"{"a":1}"#, None, false, None, 5, 26, 17, b"24"),
        (Json, br#"{"a":1}"#, None, false, Some(true), 6, 33, 17, b"24"),
        (Xml, br#"<r a="x">y</r>"#, None, false, None, 5, 10, 23, b"17"),
        (
            Multipart,
            multipart,
            Some(b"multipart/form-data; boundary=x".as_slice()),
            false,
            None,
            7,
            59,
            104,
            b"19",
        ),
        (Json, b"[", None, true, None, 4, 8, 1, b"17"),
        (Json, b"[", None, true, Some(false), 3, 7, 1, b"17"),
    ];
    for (processor, body, content_type, fallback, raw, entries, decoded, work, combined) in cases {
        let build = |limits| {
            let body = RequestBody::new(body, processor);
            if fallback {
                build_fallback_publication_with_raw_policy(body, content_type, 17, limits, raw)
            } else {
                build_publication_with_raw_policy(body, content_type, 17, limits, raw)
            }
        };
        let limits = BodyLimits {
            max_body_bytes: body.len(),
            max_entries: entries,
            max_decoded_bytes: decoded,
            max_work_bytes: work,
            ..BodyLimits::default()
        };
        let publication = build(limits).unwrap();
        assert_eq!(publication.entries.len(), entries);
        assert_eq!(
            publication.retained_bytes,
            decoded + entries * std::mem::size_of::<BodyEntry>()
        );
        assert_eq!(
            values(&publication, CollectionKind::ArgsCombinedSize),
            [(None, combined)]
        );
        if processor == Multipart {
            assert_eq!(
                values(&publication, CollectionKind::FilesCombinedSize),
                [(None, b"1".as_slice())]
            );
        }
        assert_eq!(
            build(BodyLimits {
                max_entries: entries - 1,
                ..limits
            })
            .err(),
            Some(BodyError::EntryLimit { limit: entries - 1 })
        );
        assert_eq!(
            build(BodyLimits {
                max_decoded_bytes: decoded - 1,
                ..limits
            })
            .err(),
            Some(BodyError::DecodedByteLimit { limit: decoded - 1 })
        );
        if work != 0 {
            assert_eq!(
                build(BodyLimits {
                    max_work_bytes: work - 1,
                    ..limits
                })
                .err(),
                Some(BodyError::WorkLimit { limit: work - 1 })
            );
        }
    }
}

#[test]
fn completed_json_accounting_preserves_raw_and_head_overflow_boundaries() {
    for input in [b"{}".as_slice(), b"null", br#"{"a":1}"#, b"[1,2]"] {
        let fixture = document_fixture(input, 1);
        for raw in [false, true] {
            for head in [0, 9, 99, usize::MAX] {
                for max_entries in [2, 3, 4, 5, 6, 16] {
                    let limits = BodyLimits {
                        max_entries,
                        ..BodyLimits::default()
                    };
                    let expected = build_publication_with_raw_policy(
                        RequestBody::new(input, BodyProcessorSelection::Json),
                        None,
                        head,
                        limits,
                        Some(raw),
                    );
                    let actual = validate_json_publication(&fixture.document, head, limits, raw);
                    match (actual, expected) {
                        (Ok(actual), Ok(expected)) => {
                            assert_eq!(actual.entries, expected.entries.len());
                            assert_eq!(actual.retained_bytes, expected.retained_bytes);
                        }
                        (Err(JsonPublicationError::Body(actual)), Err(expected)) => assert_eq!(actual, expected),
                        _ => panic!("completed validator and materialization disagree"),
                    }
                }
            }
        }
        assert!(fixture.document.events().is_some());
    }
    let fixture = document_fixture(br#"{"a":1}"#, 3);
    for raw in [false, true] {
        let summary = validate_json_publication(&fixture.document, 17, BodyLimits::default(), raw).unwrap();
        assert_eq!(summary.work_bytes, 17);
        assert_eq!(summary.decoded_bytes, if raw { 33 } else { 26 });
    }
}

#[test]
fn json_publication_requires_successful_completion_before_budget_checks() {
    use crate::seclang::{JsonDocument, JsonDocumentLimits, JsonStreamInput, JsonStreamLimits};
    for ending in [None, Some(JsonStreamInput::Complete), Some(JsonStreamInput::Abandoned)] {
        let mut document = JsonDocument::<()>::new(
            JsonStreamLimits::default(),
            JsonDocumentLimits {
                max_retained_bytes: 4096,
                max_key_bytes: 128,
                max_members: 32,
                max_events: 64,
                max_path_bytes: 128,
                max_depth: 8,
                max_work: 4096,
            },
        );
        document.feed(JsonStreamInput::Data(b"["), |_, _| Ok(())).unwrap();
        if let Some(ending) = ending {
            assert!(document.feed(ending, |_, _| Ok(())).is_err());
        }
        assert_eq!(
            validate_json_publication(
                &document,
                usize::MAX,
                BodyLimits {
                    max_body_bytes: 0,
                    max_entries: 0,
                    max_work_bytes: 0,
                    ..BodyLimits::default()
                },
                true
            ),
            Err(JsonPublicationError::IncompleteDocument)
        );
    }
}
