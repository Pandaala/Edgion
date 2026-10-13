use super::*;

// Preserve the historical XML publication oracle and its literal accounting
// cases. These tests do not verify the incremental XML production parser.
fn oracle_publication(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
) -> Result<BodyPublication, BodyError> {
    oracle_publication_with_raw_policy(body, content_type, head_args_combined_size, limits, None)
}

fn oracle_publication_with_raw_policy(
    body: &RequestBody,
    content_type: Option<&[u8]>,
    head_args_combined_size: usize,
    limits: BodyLimits,
    force_raw_body: Option<bool>,
) -> Result<BodyPublication, BodyError> {
    let processor = if body.processor == BodyProcessorSelection::Auto {
        processor_from_content_type(content_type)
    } else {
        body.processor
    };
    if processor != BodyProcessorSelection::Xml {
        return super::build_publication_with_raw_policy(
            body,
            content_type,
            head_args_combined_size,
            limits,
            force_raw_body,
        );
    }
    check_body_bytes(body.bytes.len(), limits)?;
    let mut builder = BodyBuilder::new(limits);
    builder.charge_work(body.bytes.len())?;
    if force_raw_body.unwrap_or(false) {
        builder.push(CollectionKind::RequestBody, None, &body.bytes, false)?;
    }
    let length = body.bytes.len().to_string();
    builder.push(CollectionKind::RequestBodyLength, None, length.as_bytes(), false)?;
    builder.push(CollectionKind::ReqbodyProcessor, None, processor.as_bytes(), false)?;
    if !body.bytes.is_empty() {
        parse_xml(&body.bytes, &mut builder)?;
    }
    let combined_size = builder.accounting.combined_size(head_args_combined_size)?.to_string();
    builder.push(CollectionKind::ArgsCombinedSize, None, combined_size.as_bytes(), false)?;
    builder.finish()
}

fn publication(bytes: &[u8], processor: BodyProcessorSelection, content_type: Option<&[u8]>) -> BodyPublication {
    oracle_publication(
        &RequestBody::new(bytes::Bytes::copy_from_slice(bytes), processor),
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
            oracle_publication(
                &RequestBody::new(bytes::Bytes::copy_from_slice(&body), BodyProcessorSelection::Multipart),
                Some(&content_type),
                0,
                BodyLimits {
                    max_work_bytes: body.len(),
                    ..BodyLimits::default()
                }
            ),
            Err(BodyError::WorkLimit { .. })
        ));
    }
    let too_long = [b'a'; 201];
    let mut content_type = b"multipart/form-data; boundary=".to_vec();
    content_type.extend_from_slice(&too_long);
    assert!(matches!(
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(b"--x--\r\n"),
                BodyProcessorSelection::Multipart
            ),
            Some(&content_type),
            0,
            BodyLimits::default()
        ),
        Err(BodyError::MultipartBoundary)
    ));
    assert!(matches!(
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nv"),
                BodyProcessorSelection::Multipart
            ),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits::default()
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
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(b"<!DOCTYPE x><x/>"),
                BodyProcessorSelection::Xml
            ),
            None,
            0,
            BodyLimits::default()
        ),
        Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml
        })
    ));
}

#[test]
fn body_limits_accept_exact_and_reject_first_excess_without_content() {
    let exact = oracle_publication(
        &RequestBody::new(bytes::Bytes::copy_from_slice(b"abc"), BodyProcessorSelection::Raw),
        None,
        0,
        BodyLimits {
            max_body_bytes: 3,
            ..BodyLimits::default()
        },
    )
    .unwrap();
    assert!(!exact.entries.is_empty());
    let Err(error) = oracle_publication(
        &RequestBody::new(bytes::Bytes::copy_from_slice(b"private"), BodyProcessorSelection::Raw),
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
    let raw = RequestBody::new(bytes::Bytes::copy_from_slice(b"abc"), BodyProcessorSelection::Raw);
    assert!(oracle_publication(
        &raw,
        None,
        0,
        BodyLimits {
            max_entries: 4,
            max_decoded_bytes: 8,
            max_work_bytes: 3,
            ..BodyLimits::default()
        }
    )
    .is_ok());
    assert!(matches!(
        oracle_publication(
            &raw,
            None,
            0,
            BodyLimits {
                max_entries: 3,
                ..BodyLimits::default()
            }
        ),
        Err(BodyError::EntryLimit { limit: 3 })
    ));
    assert!(matches!(
        oracle_publication(
            &raw,
            None,
            0,
            BodyLimits {
                max_decoded_bytes: 7,
                ..BodyLimits::default()
            }
        ),
        Err(BodyError::DecodedByteLimit { limit: 7 })
    ));
    assert!(matches!(
        oracle_publication(
            &raw,
            None,
            0,
            BodyLimits {
                max_work_bytes: 2,
                ..BodyLimits::default()
            }
        ),
        Err(BodyError::WorkLimit { limit: 2 })
    ));
    assert!(matches!(
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(b"name=value"),
                BodyProcessorSelection::UrlEncoded
            ),
            None,
            0,
            BodyLimits {
                max_name_bytes: 3,
                ..BodyLimits::default()
            }
        ),
        Err(BodyError::NameByteLimit { limit: 3 })
    ));
    let multipart = concat!(
        "--x\r\n",
        "Content-Disposition: form-data; name=\"a\"\r\n",
        "X-Test: one\r\n\r\n",
        "1\r\n",
        "--x--\r\n"
    );
    assert!(matches!(
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(multipart.as_bytes()),
                BodyProcessorSelection::Multipart
            ),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits {
                max_multipart_parts: 0,
                ..BodyLimits::default()
            }
        ),
        Err(BodyError::MultipartPartLimit { limit: 0 })
    ));
    assert!(matches!(
        oracle_publication(
            &RequestBody::new(
                bytes::Bytes::copy_from_slice(multipart.as_bytes()),
                BodyProcessorSelection::Multipart
            ),
            Some(b"multipart/form-data; boundary=x"),
            0,
            BodyLimits {
                max_multipart_headers: 1,
                ..BodyLimits::default()
            }
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
            oracle_publication(
                &RequestBody::new(bytes::Bytes::copy_from_slice(body), BodyProcessorSelection::Xml),
                None,
                0,
                BodyLimits::default()
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
    let body = RequestBody::new(
        bytes::Bytes::copy_from_slice(b"private-secret"),
        BodyProcessorSelection::Json,
    );
    let rendered = format!("{body:?}");
    assert!(!rendered.contains("private"));
    assert!(rendered.contains("14"));
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
            let body = RequestBody::new(bytes::Bytes::copy_from_slice(body), processor);
            if fallback {
                build_fallback_publication_with_raw_policy(&body, content_type, 17, limits, raw)
            } else {
                oracle_publication_with_raw_policy(&body, content_type, 17, limits, raw)
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

fn parse_xml(input: &[u8], builder: &mut BodyBuilder) -> Result<(), BodyError> {
    if input.windows(9).any(|window| window.eq_ignore_ascii_case(b"<!DOCTYPE"))
        || input.windows(8).any(|window| window.eq_ignore_ascii_case(b"<!ENTITY"))
    {
        return Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        });
    }
    let mut cursor = 0_usize;
    let mut elements: Vec<Vec<u8>> = Vec::new();
    let mut roots = 0_usize;
    while cursor < input.len() {
        let Some(open) = input[cursor..].iter().position(|byte| *byte == b'<') else {
            let text = trim_ascii(&input[cursor..]);
            if !text.is_empty() {
                if elements.is_empty() {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                }
                builder.push(CollectionKind::Xml, None, &decode_xml(text)?, false)?;
            }
            break;
        };
        let text_end = cursor.checked_add(open).ok_or(BodyError::Arithmetic)?;
        let text = trim_ascii(&input[cursor..text_end]);
        if !text.is_empty() {
            if elements.is_empty() {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            builder.push(CollectionKind::Xml, None, &decode_xml(text)?, false)?;
        }
        if input.get(text_end..text_end + 4) == Some(b"<!--") {
            let Some(end) = find_bytes(&input[text_end + 4..], b"-->") else {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            };
            cursor = text_end + 4 + end + 3;
            continue;
        }
        if input.get(text_end..text_end + 9) == Some(b"<![CDATA[") {
            if elements.is_empty() {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            let Some(end) = find_bytes(&input[text_end + 9..], b"]]>") else {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            };
            builder.push(
                CollectionKind::Xml,
                None,
                &input[text_end + 9..text_end + 9 + end],
                false,
            )?;
            cursor = text_end + 9 + end + 3;
            continue;
        }
        let Some(close) = input[text_end..].iter().position(|byte| *byte == b'>') else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        };
        let tag_end = text_end.checked_add(close).ok_or(BodyError::Arithmetic)?;
        let tag = trim_ascii(&input[text_end + 1..tag_end]);
        if tag.starts_with(b"?") {
            cursor = tag_end + 1;
            continue;
        }
        if tag.starts_with(b"/") {
            let closing = trim_ascii(&tag[1..]);
            let opening = elements.pop().ok_or(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            })?;
            if closing != opening {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
        } else {
            let self_closing = tag.ends_with(b"/");
            let opening = trim_ascii(tag.strip_suffix(b"/").unwrap_or(tag));
            let name_end = opening
                .iter()
                .position(u8::is_ascii_whitespace)
                .unwrap_or(opening.len());
            let name = &opening[..name_end];
            if name.is_empty() || name.iter().copied().any(|byte| b"<>&\"'=/".contains(&byte)) {
                return Err(BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                });
            }
            if elements.is_empty() {
                roots = roots.checked_add(1).ok_or(BodyError::Arithmetic)?;
                if roots > 1 {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                }
            }
            let depth = elements.len().checked_add(1).ok_or(BodyError::Arithmetic)?;
            if depth > builder.accounting.limits.max_depth {
                return Err(BodyError::DepthLimit {
                    limit: builder.accounting.limits.max_depth,
                });
            }
            parse_xml_attributes(opening, builder)?;
            if !self_closing {
                elements.try_reserve(1).map_err(|_| BodyError::Allocation)?;
                elements.push(try_copy(name)?);
            }
        }
        builder.charge_work(tag.len())?;
        cursor = tag_end + 1;
    }
    if !elements.is_empty() || roots != 1 {
        return Err(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        });
    }
    Ok(())
}

fn parse_xml_attributes(tag: &[u8], builder: &mut BodyBuilder) -> Result<(), BodyError> {
    let mut cursor = tag.iter().position(u8::is_ascii_whitespace).unwrap_or(tag.len());
    while cursor < tag.len() {
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == tag.len() {
            break;
        }
        let start = cursor;
        while cursor < tag.len() && !tag[cursor].is_ascii_whitespace() && tag[cursor] != b'=' {
            cursor += 1;
        }
        let name = &tag[start..cursor];
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if tag.get(cursor) != Some(&b'=') {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        }
        cursor += 1;
        while cursor < tag.len() && tag[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *tag.get(cursor).ok_or(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        })?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < tag.len() && tag[cursor] != quote {
            cursor += 1;
        }
        let value = tag.get(value_start..cursor).ok_or(BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        })?;
        cursor = cursor.checked_add(1).ok_or(BodyError::Arithmetic)?;
        builder.push(CollectionKind::Xml, Some(name), &decode_xml(value)?, true)?;
    }
    Ok(())
}

fn decode_xml(value: &[u8]) -> Result<Vec<u8>, BodyError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| BodyError::Allocation)?;
    let mut cursor = 0_usize;
    while cursor < value.len() {
        if value[cursor] != b'&' {
            output.push(value[cursor]);
            cursor += 1;
            continue;
        }
        let Some(relative_end) = value[cursor..].iter().position(|byte| *byte == b';') else {
            return Err(BodyError::Malformed {
                processor: BodyProcessorSelection::Xml,
            });
        };
        let end = cursor + relative_end;
        let entity = &value[cursor + 1..end];
        match entity {
            b"amp" => output.push(b'&'),
            b"lt" => output.push(b'<'),
            b"gt" => output.push(b'>'),
            b"quot" => output.push(b'"'),
            b"apos" => output.push(b'\''),
            _ => {
                let (digits, radix) = if let Some(digits) = entity.strip_prefix(b"#x") {
                    (digits, 16)
                } else if let Some(digits) = entity.strip_prefix(b"#") {
                    (digits, 10)
                } else {
                    return Err(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    });
                };
                let digits = std::str::from_utf8(digits).map_err(|_| BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                })?;
                let scalar = u32::from_str_radix(digits, radix)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|value| is_xml_scalar(*value))
                    .ok_or(BodyError::Malformed {
                        processor: BodyProcessorSelection::Xml,
                    })?;
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(scalar.encode_utf8(&mut encoded).as_bytes());
            }
        }
        cursor = end + 1;
    }
    Ok(output)
}

fn is_xml_scalar(value: char) -> bool {
    let value = u32::from(value);
    matches!(value, 0x09 | 0x0A | 0x0D)
        || (0x20..=0xD7FF).contains(&value)
        || (0xE000..=0xFFFD).contains(&value)
        || (0x10000..=0x10FFFF).contains(&value)
}

#[test]
fn form_decoder_borrows_plain_bytes_and_keeps_tolerant_owned_decoding() {
    for input in [b"".as_slice(), b"item", b"from-body", b"binary\0\xff"] {
        let decoded = percent_decode(input).unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded.as_ref(), input);
        assert_eq!(decoded.as_ptr(), input.as_ptr());
    }
    for (input, expected) in [
        (b"a+b".as_slice(), b"a b".as_slice()),
        (b"%2B+%2b", b"+ +"),
        (b"%2520", b"%20"),
        (b"%Q0%", b"%Q0%"),
        (b"%2", b"%2"),
        (b"%00%FF", b"\0\xff"),
        (b"a%26b%3Dc", b"a&b=c"),
    ] {
        let decoded = percent_decode(input).unwrap();
        assert!(matches!(decoded, Cow::Owned(_)));
        assert_eq!(decoded.as_ref(), expected);
    }
}

#[test]
fn form_publication_owns_fields_after_input_drop_and_preserves_split_order() {
    let published = {
        let input = b"&item=from-body&&item=second=tail&flag&=empty-name&plus=%2B+&".to_vec();
        publication(&input, BodyProcessorSelection::UrlEncoded, None)
    };
    assert_eq!(
        values(&published, CollectionKind::Args),
        [
            (Some(b"item".as_slice()), b"from-body".as_slice()),
            (Some(b"item".as_slice()), b"second=tail".as_slice()),
            (Some(b"flag".as_slice()), b"".as_slice()),
            (Some(b"".as_slice()), b"empty-name".as_slice()),
            (Some(b"plus".as_slice()), b"+ ".as_slice()),
        ]
    );
    assert_eq!(
        values(&published, CollectionKind::ArgsNames),
        [
            (Some(b"item".as_slice()), b"item".as_slice()),
            (Some(b"item".as_slice()), b"item".as_slice()),
            (Some(b"flag".as_slice()), b"flag".as_slice()),
            (Some(b"".as_slice()), b"".as_slice()),
            (Some(b"plus".as_slice()), b"plus".as_slice()),
        ]
    );
}
