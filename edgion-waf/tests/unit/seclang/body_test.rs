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
