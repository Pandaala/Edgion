use super::*;
#[derive(Default)]
struct Sink(Vec<(Option<Vec<u8>>, Vec<u8>)>);
impl XmlStreamSink for Sink {
    fn value(&mut self, name: Option<&[u8]>, value: &[u8]) -> Result<(), XmlStreamSinkError> {
        self.0.push((name.map(<[u8]>::to_vec), value.to_vec()));
        Ok(())
    }
}
fn parse(input: &[u8], chunk: usize) -> Result<(Sink, XmlStreamUsage, XmlSemanticSummary), XmlStreamError> {
    let mut stream = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    for bytes in input.chunks(chunk) {
        stream.feed(XmlStreamInput::Data(bytes), &mut sink)?;
    }
    stream.feed(XmlStreamInput::Complete, &mut sink)?;
    Ok((sink, stream.usage(), stream.summary()))
}
#[test]
fn all_splits_preserve_normalized_projection_and_namespace_order() {
    let input = b"\xEF\xBB\xBF<?xml version='1.0' encoding='UTF-8'?><p:r p:a='x>y' xmlns:p='urn:p'>  A&amp;B \r\n<![CDATA[ raw\r\n]]><!-- <!DOCTYPE harmless> --><?ok data?><i/>tail </p:r>";
    let expected = parse(input, input.len()).unwrap();
    for chunk in 1..=input.len() {
        let result = parse(input, chunk).unwrap();
        assert_eq!(result.0 .0, expected.0 .0);
        assert_eq!(result.2, expected.2, "chunk {chunk}");
    }
    assert_eq!(
        expected.0 .0,
        vec![
            (Some(b"p:a".to_vec()), b"x>y".to_vec()),
            (Some(b"xmlns:p".to_vec()), b"urn:p".to_vec()),
            // One element value: text, CDATA and later text concatenated,
            // comment and PI removed, outer literal whitespace trimmed.
            (None, b"A&B \n raw\ntail".to_vec())
        ]
    );
}
#[test]
fn strict_invalid_documents_are_sticky() {
    for input in [
        b"<r a='x' a='y'/>".as_slice(),
        b"<r a=x/>",
        b"<r/ >",
        b"<r>]]></r>",
        b"<r>x]]>y</r>",
        b"<r><!-- a--b --></r>",
        b"<r>&#0;</r>",
        b"<r>&#xD800;</r>",
        b"<p:r/>",
        b"<r xmlns:xml='bad'/>",
        b"<r xmlns:p='u' xmlns:q='u' p:a='x' q:a='y'/>",
        b"<!--\xFF--><r/>",
        b" <r/><?xml version='1.0'?>",
        b"<!DOCTYPE r><r/>",
        b"<1r/>",
        b"<r>",
    ] {
        assert!(parse(input, 1).is_err(), "{input:?}");
        let mut stream = XmlStream::new(XmlStreamLimits::default());
        let mut sink = Sink::default();
        let error = stream
            .feed(XmlStreamInput::Data(input), &mut sink)
            .and_then(|()| stream.feed(XmlStreamInput::Complete, &mut sink))
            .unwrap_err();
        assert_eq!(stream.feed(XmlStreamInput::Data(b"<r/>"), &mut sink), Err(error));
    }
}
#[test]
fn attribute_borrowing_and_reusable_content_capacity() {
    struct Borrowed {
        source: usize,
        end: usize,
        values: usize,
    }
    impl XmlStreamSink for Borrowed {
        fn value(&mut self, name: Option<&[u8]>, value: &[u8]) -> Result<(), XmlStreamSinkError> {
            // Attribute values stay borrowed; element text is published from
            // the content buffer at the element's close.
            let pointer = value.as_ptr() as usize;
            assert_eq!(
                pointer >= self.source && pointer + value.len() <= self.end,
                name.is_some()
            );
            self.values += 1;
            Ok(())
        }
    }
    let input = b"<r a='plain'>ordinary</r>";
    let mut sink = Borrowed {
        source: input.as_ptr() as usize,
        end: input.as_ptr() as usize + input.len(),
        values: 0,
    };
    let mut stream = XmlStream::new(XmlStreamLimits::default());
    stream.feed(XmlStreamInput::Data(input), &mut sink).unwrap();
    stream.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(sink.values, 2);
    let mut stream = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    stream.feed(XmlStreamInput::Data(b"<r>"), &mut sink).unwrap();
    stream.feed(XmlStreamInput::Data(&vec![b'x'; 4096]), &mut sink).unwrap();
    assert!(stream.content.spilled());
    assert_eq!(stream.token.capacity(), 1024);
    stream.feed(XmlStreamInput::Data(b"<i/>small<i/>"), &mut sink).unwrap();
    stream.feed(XmlStreamInput::Data(b"</r>"), &mut sink).unwrap();
    // The root close releases the content but keeps its capacity.
    let capacity = stream.content.capacity();
    assert!(stream.content.is_empty());
    assert!(capacity >= 4096 + 5);
    stream.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(stream.content.capacity(), capacity);
    assert_eq!(sink.0.len(), 1);
    assert_eq!(sink.0[0].1.len(), 4096 + 5);
}
#[test]
fn references_whitespace_and_empty_cdata() {
    let result = parse(b"<r a='\t&#9;\r\n'> &#32; x &#32; <![CDATA[]]></r>", 1).unwrap();
    assert_eq!(
        result.0 .0,
        vec![
            (Some(b"a".to_vec()), b" \t ".to_vec()),
            // Only the leading literal space is outer; the space before the
            // (empty) CDATA section is interior.
            (None, b"  x   ".to_vec())
        ]
    );
}
#[test]
fn parser_caps_charge_unselected_content_and_structural_events() {
    type Setter = fn(&mut XmlStreamLimits, usize);
    let cases: &[(XmlStreamLimit, Setter, usize, &[u8])] = &[
        (XmlStreamLimit::SourceBytes, |l, n| l.max_source_bytes = n, 4, b"<r/>"),
        (
            XmlStreamLimit::RawTokenBytes,
            |l, n| l.max_raw_token_bytes = n,
            8,
            b"<!--x--><r/>",
        ),
        (XmlStreamLimit::NameBytes, |l, n| l.max_name_bytes = n, 3, b"<abc/>"),
        (
            XmlStreamLimit::DecodedBytes,
            |l, n| l.max_decoded_bytes = n,
            3,
            b"<r>abc</r>",
        ),
        // Four tag events plus the root's single (empty) text value.
        (XmlStreamLimit::Events, |l, n| l.max_events = n, 5, b"<r><i/></r>"),
        (XmlStreamLimit::Depth, |l, n| l.max_depth = n, 2, b"<r><i/></r>"),
        (
            XmlStreamLimit::Attributes,
            |l, n| l.max_attributes = n,
            2,
            b"<r a='' b=''/>",
        ),
        (
            XmlStreamLimit::Namespaces,
            |l, n| l.max_namespaces = n,
            2,
            b"<r xmlns:a='a' xmlns:b='b'/>",
        ),
    ];
    for &(kind, set, exact, input) in cases {
        for chunk in [1, input.len()] {
            for (bound, succeeds) in [(exact, true), (exact - 1, false)] {
                let mut limits = XmlStreamLimits::default();
                set(&mut limits, bound);
                let mut stream = XmlStream::new(limits);
                let mut sink = Sink::default();
                let result = input
                    .chunks(chunk)
                    .try_for_each(|data| stream.feed(XmlStreamInput::Data(data), &mut sink))
                    .and_then(|()| stream.feed(XmlStreamInput::Complete, &mut sink));
                if succeeds {
                    assert!(result.is_ok(), "{kind:?} {chunk}: {result:?}");
                } else {
                    assert_eq!(
                        result,
                        Err(XmlStreamError::Limit { kind, limit: bound }),
                        "chunk {chunk}"
                    );
                }
            }
        }
    }
}
#[test]
fn tag_only_identity_is_reclaimed_and_attribute_values_are_borrowed() {
    let mut stream = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    stream
        .feed(
            XmlStreamInput::Data(b"<r a='value' b='value' xmlns:p='urn:p'>"),
            &mut sink,
        )
        .unwrap();
    assert_eq!(stream.arena.as_slice(), b"rpurn:p");
    let plateau = stream.usage().structure_bytes;
    for _ in 0..1000 {
        stream.feed(XmlStreamInput::Data(b"<p:i a='x'/>"), &mut sink).unwrap();
        assert_eq!(stream.usage().structure_bytes, plateau);
    }
    stream.feed(XmlStreamInput::Data(b"</r>"), &mut sink).unwrap();
    stream.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(stream.usage().structure_bytes, 0);
    for input in [b"<r a='plain'/>".as_slice(), b"<r a=''/>"] {
        struct Borrow<'a>(&'a [u8]);
        impl XmlStreamSink for Borrow<'_> {
            fn value(&mut self, name: Option<&[u8]>, value: &[u8]) -> Result<(), XmlStreamSinkError> {
                // The root's empty text value comes from the content buffer.
                if name.is_some() {
                    let p = value.as_ptr() as usize;
                    assert!(
                        p >= self.0.as_ptr() as usize && p + value.len() <= self.0.as_ptr() as usize + self.0.len()
                    );
                }
                Ok(())
            }
        }
        let mut parser = XmlStream::new(XmlStreamLimits::default());
        let mut sink = Borrow(input);
        parser.feed(XmlStreamInput::Data(input), &mut sink).unwrap();
        parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    }
    // CDATA values are copied into element content, including the empty one.
    for (input, value) in [
        (b"<r><![CDATA[plain]]></r>".as_slice(), b"plain".as_slice()),
        (b"<r><![CDATA[]]></r>", b""),
    ] {
        assert_eq!(parse(input, input.len()).unwrap().0 .0, [(None, value.to_vec())]);
    }
    assert!(parse(b"\xEF\xBB\xBF <?xml version='1.0'?><r/>", 999).is_err());
    assert!(parse(b"<r>]]&gt;</r>", 1).is_ok());
}
#[test]
fn every_two_part_cut_with_empty_feed_preserves_valid_and_invalid_results() {
    for input in [
        b"<r a='plain'><![CDATA[]]> x&#32; </r>".as_slice(),
        b"\xEF\xBB\xBF <?xml version='1.0'?><r/>",
        b"<r>]]&gt;</r>",
        b"<r a='\xC3'>x</r>",
    ] {
        let expected = parse(input, input.len());
        for cut in 0..=input.len() {
            let mut parser = XmlStream::new(XmlStreamLimits::default());
            let mut sink = Sink::default();
            let result = parser
                .feed(XmlStreamInput::Data(&input[..cut]), &mut sink)
                .and_then(|()| parser.feed(XmlStreamInput::Data(b""), &mut sink))
                .and_then(|()| parser.feed(XmlStreamInput::Data(&input[cut..]), &mut sink))
                .and_then(|()| parser.feed(XmlStreamInput::Complete, &mut sink));
            match &expected {
                Ok((values, _, summary)) => {
                    assert!(result.is_ok(), "cut {cut}: {result:?}");
                    assert_eq!(sink.0, values.0);
                    assert_eq!(parser.summary(), *summary);
                }
                Err(error) => assert_eq!(result, Err(*error), "cut {cut}"),
            }
        }
    }
}
#[test]
fn namespace_reserved_bindings_default_scope_and_large_sorted_attributes() {
    for input in [
        b"<r xmlns='u' xmlns:p='u' x='a' p:x='b'/>".as_slice(),
        b"<r xml:lang='en'/>",
        b"<r xmlns:xml='http://www.w3.org/XML/1998/namespace'/>",
        b"<r xmlns:p='u'><p:i xmlns:p='v'/><p:i/></r>",
    ] {
        assert!(parse(input, 1).is_ok(), "{input:?}");
    }
    for input in [
        b"<r xmlns='http://www.w3.org/XML/1998/namespace'/>".as_slice(),
        b"<r xmlns:xmlns='u'/>",
        b"<r xmlns:p=''/>",
        b"<r a:b:c='x'/>",
    ] {
        assert!(parse(input, 1).is_err(), "{input:?}");
    }
    let mut input = b"<r ".to_vec();
    for i in (0..10_000).rev() {
        input.extend_from_slice(format!("a{i:05}='x' ").as_bytes());
    }
    input.extend_from_slice(b"/>");
    let result = parse(&input, input.len()).unwrap();
    // 10,000 attributes plus the root's empty text value.
    assert_eq!(result.2.value_count, 10_001);
    assert!(result.1.work < 16 << 20, "charged work {}", result.1.work);
}
#[test]
fn structure_heap_and_work_exact_bounds_are_independent_and_held_scratch_is_charged() {
    let input = b"<r a='x'/>";
    let (_, usage, _) = parse(input, input.len()).unwrap();
    for (kind, exact) in [
        (XmlStreamLimit::Work, usage.work),
        (XmlStreamLimit::RetainedBytes, usage.peak_retained_bytes),
        (XmlStreamLimit::StructureBytes, usage.peak_structure_bytes),
    ] {
        for succeeds in [true, false] {
            let bound = exact - usize::from(!succeeds);
            let mut limits = XmlStreamLimits::default();
            match kind {
                XmlStreamLimit::Work => limits.max_work = bound,
                XmlStreamLimit::RetainedBytes => limits.max_retained_bytes = bound,
                XmlStreamLimit::StructureBytes => limits.max_structure_bytes = bound,
                _ => unreachable!(),
            };
            let mut parser = XmlStream::new(limits);
            let mut sink = Sink::default();
            let result = parser
                .feed(XmlStreamInput::Data(input), &mut sink)
                .and_then(|()| parser.feed(XmlStreamInput::Complete, &mut sink));
            if succeeds {
                assert!(result.is_ok(), "{kind:?}: {result:?}");
            } else {
                assert_eq!(result, Err(XmlStreamError::Limit { kind, limit: bound }));
            }
        }
    }
    let mut input = b"<".to_vec();
    input.extend_from_slice(&vec![b'a'; 1025]);
    input.extend_from_slice(b"/>");
    let limits = XmlStreamLimits {
        max_structure_bytes: 2049,
        ..XmlStreamLimits::default()
    };
    let mut parser = XmlStream::new(limits);
    let mut sink = Sink::default();
    assert_eq!(
        parser.feed(XmlStreamInput::Data(&input), &mut sink),
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::StructureBytes,
            limit: 2049
        })
    );
}

#[test]
fn whitespace_chunks_are_linear_and_probes_stop_at_admission_limits() {
    let spaces = vec![b' '; 20_000];
    let input = [b"<r>".as_slice(), &spaces, b"x</r>"].concat();
    let expected = parse(&input, input.len()).unwrap();
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    for part in [b"<r>".as_slice(), &spaces, b"x</r>"] {
        parser.feed(XmlStreamInput::Data(part), &mut sink).unwrap();
    }
    parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(sink.0, expected.0 .0);
    assert_eq!(parser.summary(), expected.2);
    assert!(parser.usage().work < input.len() * 6);
    let external = [spaces.as_slice(), b"<r/>", &spaces].concat();
    let parsed = parse(&external, external.len()).unwrap();
    // Outer whitespace is not content; the root publishes one empty value.
    assert_eq!(parsed.2.value_count, 1);
    assert!(parsed.1.work < external.len() * 6);
    for kind in [
        XmlStreamLimit::SourceBytes,
        XmlStreamLimit::RawTokenBytes,
        XmlStreamLimit::Work,
    ] {
        let mut limits = XmlStreamLimits::default();
        match kind {
            XmlStreamLimit::SourceBytes => limits.max_source_bytes = 10,
            XmlStreamLimit::RawTokenBytes => limits.max_raw_token_bytes = 10,
            XmlStreamLimit::Work => limits.max_work = 32,
            _ => unreachable!(),
        }
        let mut parser = XmlStream::new(limits);
        let mut sink = Sink::default();
        parser.feed(XmlStreamInput::Data(b"<r>"), &mut sink).unwrap();
        let error = parser.feed(XmlStreamInput::Data(&spaces), &mut sink).unwrap_err();
        assert!(matches!(error,XmlStreamError::Limit{kind:k,..} if k==kind));
        assert!(parser.usage().work <= 33);
        assert!(parser.usage().source_bytes <= 10);
        assert_eq!(parser.feed(XmlStreamInput::Complete, &mut sink), Err(error));
    }
}
#[test]
fn pi_grammar_fixed_markup_and_definitive_decoded_bytes_have_independent_caps() {
    for input in [b"<?p?><r/>".as_slice(), b"<?p ?data?><r/>"] {
        for chunk in 1..=input.len() {
            assert!(parse(input, chunk).is_ok());
        }
    }
    for input in [b"<?p?data?><r/>".as_slice(), b"<?p??><r/>"] {
        for chunk in 1..=input.len() {
            assert_eq!(parse(input, chunk).err(), Some(XmlStreamError::Syntax));
        }
    }
    for input in [b"<r><!--x--></r>".as_slice(), b"<r><![CDATA[x]]></r>"] {
        let mut parser = XmlStream::new(XmlStreamLimits {
            max_name_bytes: 1,
            ..Default::default()
        });
        let mut sink = Sink::default();
        for byte in input {
            parser
                .feed(XmlStreamInput::Data(std::slice::from_ref(byte)), &mut sink)
                .unwrap();
        }
        parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    }
    let mut parser = XmlStream::new(XmlStreamLimits {
        max_decoded_bytes: 0,
        ..Default::default()
    });
    let mut sink = Sink::default();
    parser.feed(XmlStreamInput::Data(b"<r>"), &mut sink).unwrap();
    assert_eq!(
        parser.feed(XmlStreamInput::Data(&vec![b'x'; 4096]), &mut sink),
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::DecodedBytes,
            limit: 0
        })
    );
    assert!(!parser.token.spilled());
    assert_eq!(parser.usage().decoded_bytes, 0);
    for (input, bytes) in [
        (b"<r>   </r>".as_slice(), 0usize),
        (b"<r>x   </r>", 1),
        (b"<r>x   y</r>", 5),
        (b"<r>&#32;</r>", 1),
        (b"<r><![CDATA[ ]]></r>", 1),
        (b"<r a=' '/>", 1),
    ] {
        for cap in [bytes, bytes.saturating_sub(1)] {
            let mut parser = XmlStream::new(XmlStreamLimits {
                max_decoded_bytes: cap,
                ..Default::default()
            });
            let mut sink = Sink::default();
            let result = input
                .iter()
                .try_for_each(|byte| parser.feed(XmlStreamInput::Data(std::slice::from_ref(byte)), &mut sink))
                .and_then(|()| parser.feed(XmlStreamInput::Complete, &mut sink));
            assert_eq!(result.is_ok(), cap == bytes, "{input:?}/{cap}");
            if cap == bytes {
                assert_eq!(parser.usage().decoded_bytes, bytes);
            }
        }
    }
}
#[test]
fn concatenated_root_content_crosses_caps_that_no_single_segment_crosses() {
    fn feed_bytewise(input: &[u8], limits: XmlStreamLimits) -> (Result<(), XmlStreamError>, XmlStreamUsage) {
        let mut parser = XmlStream::new(limits);
        let mut sink = Sink::default();
        let result = input
            .iter()
            .try_for_each(|byte| parser.feed(XmlStreamInput::Data(std::slice::from_ref(byte)), &mut sink))
            .and_then(|()| parser.feed(XmlStreamInput::Complete, &mut sink));
        (result, parser.usage())
    }
    // Each row's longest single text/CDATA segment fits `bytes - 1`; only the
    // concatenation across a comment or PI (with interior whitespace kept)
    // reaches `bytes`.
    for (input, bytes, longest_segment) in [
        (b"<r>ab<!--c-->cd</r>".as_slice(), 4usize, 2usize),
        (b"<r>a <!--c--> b</r>", 4, 2),
        (b"<r><![CDATA[a]]><?p?><![CDATA[b]]></r>", 2, 1),
        (b"<r>ab<?p x?><![CDATA[cd]]><!--c-->ef</r>", 6, 2),
    ] {
        assert!(longest_segment < bytes, "{input:?}");
        for cap in [bytes, bytes - 1] {
            let (result, usage) = feed_bytewise(
                input,
                XmlStreamLimits {
                    max_decoded_bytes: cap,
                    ..Default::default()
                },
            );
            if cap == bytes {
                assert_eq!(result, Ok(()), "{input:?}");
                assert_eq!(usage.decoded_bytes, bytes, "{input:?}");
            } else {
                assert_eq!(
                    result,
                    Err(XmlStreamError::Limit {
                        kind: XmlStreamLimit::DecodedBytes,
                        limit: cap
                    }),
                    "{input:?}"
                );
            }
        }
    }
    // Retained bytes: two CDATA sections around a comment. One section alone
    // fits the cap; the concatenated root content does not.
    let section = vec![b'x'; 600];
    let mut single = b"<r><![CDATA[".to_vec();
    single.extend_from_slice(&section);
    single.extend_from_slice(b"]]></r>");
    let mut split = b"<r><![CDATA[".to_vec();
    split.extend_from_slice(&section);
    split.extend_from_slice(b"]]><!--c--><![CDATA[");
    split.extend_from_slice(&section);
    split.extend_from_slice(b"]]></r>");
    // The smallest admitted cap is not the unlimited peak because growth is
    // clamped to the remaining budget, so search for it.
    let minimal_cap = |input: &[u8]| {
        let (result, usage) = feed_bytewise(input, XmlStreamLimits::default());
        assert_eq!(result, Ok(()));
        let (mut low, mut high) = (0, usage.peak_retained_bytes);
        while low < high {
            let mid = low + (high - low) / 2;
            let limits = XmlStreamLimits {
                max_retained_bytes: mid,
                ..Default::default()
            };
            if feed_bytewise(input, limits).0.is_ok() {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        low
    };
    let cap = minimal_cap(&split) - 1;
    assert!(minimal_cap(&single) <= cap, "single section must fit {cap}");
    let limits = XmlStreamLimits {
        max_retained_bytes: cap,
        ..Default::default()
    };
    assert_eq!(feed_bytewise(&single, limits).0, Ok(()));
    assert_eq!(
        feed_bytewise(&split, limits).0,
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::RetainedBytes,
            limit: cap
        })
    );
}
#[test]
fn every_utf8_partition_cdata_tail_and_terminal_sink_state_are_checked() {
    for input in [
        "<é:r xmlns:é='u' é:ñ='€'>π</é:r>".as_bytes(),
        b"<r a='\xC0\xAF'/>",
        b"<r><!--\xED\xA0\x80--></r>",
    ] {
        let expected = parse(input, input.len());
        for cut in 0..=input.len() {
            let mut parser = XmlStream::new(XmlStreamLimits::default());
            let mut sink = Sink::default();
            let result = parser
                .feed(XmlStreamInput::Data(&input[..cut]), &mut sink)
                .and_then(|()| parser.feed(XmlStreamInput::Data(&input[cut..]), &mut sink))
                .and_then(|()| parser.feed(XmlStreamInput::Complete, &mut sink));
            match &expected {
                Ok((values, _, summary)) => {
                    result.unwrap();
                    assert_eq!(sink.0, values.0);
                    assert_eq!(parser.summary(), *summary)
                }
                Err(error) => assert_eq!(result, Err(*error)),
            }
        }
    }
    let input = format!("<r><![CDATA[{}]]></r>", "]".repeat(4096));
    let expected = parse(input.as_bytes(), input.len()).unwrap();
    for chunk in [1, 1023, 1024, 1025] {
        let actual = parse(input.as_bytes(), chunk).unwrap();
        assert_eq!(actual.0 .0, expected.0 .0);
        assert_eq!(actual.1.decoded_bytes, 4096);
    }
    struct Reject;
    impl XmlStreamSink for Reject {
        fn value(&mut self, _: Option<&[u8]>, _: &[u8]) -> Result<(), XmlStreamSinkError> {
            Err(XmlStreamSinkError)
        }
    }
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    assert_eq!(
        parser.feed(XmlStreamInput::Data(b"<r>x</r>"), &mut Reject),
        Err(XmlStreamError::SinkRejected)
    );
    assert_eq!(
        parser.feed(XmlStreamInput::Complete, &mut Reject),
        Err(XmlStreamError::SinkRejected)
    );
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    assert_eq!(
        parser.feed(XmlStreamInput::Abandoned, &mut sink),
        Err(XmlStreamError::Abandoned)
    );
    assert_eq!(
        parser.feed(XmlStreamInput::Data(b"<r/>"), &mut sink),
        Err(XmlStreamError::Abandoned)
    );
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    parser.feed(XmlStreamInput::Data(b"<r/>"), &mut sink).unwrap();
    parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
    assert_eq!(
        parser.feed(XmlStreamInput::Complete, &mut sink),
        Err(XmlStreamError::AlreadyComplete)
    );
}

#[test]
fn growth_copy_work_is_admitted_before_allocation_and_usage_tracks_real_capacity() {
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    let mut sink = Sink::default();
    parser.feed(XmlStreamInput::Data(b"<r>"), &mut sink).unwrap();
    parser.feed(XmlStreamInput::Data(&vec![b'x'; 1024]), &mut sink).unwrap();
    let heap = parser.heap().unwrap();
    let capacity = parser.content.capacity();
    parser.limits.max_work = parser.usage().work + 1;
    assert!(matches!(
        parser.feed(XmlStreamInput::Data(b"x"), &mut sink),
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::Work,
            ..
        })
    ));
    assert_eq!(parser.content.capacity(), capacity);
    assert!(!parser.content.spilled());
    assert_eq!(parser.usage().retained_bytes, heap);
    assert_eq!(parser.heap().unwrap(), heap);
    {
        let mut bytes = SmallVec::<[u8; 256]>::from_slice(&[b'x'; 256]);
        let mut usage = XmlStreamUsage::default();
        let before = bytes.capacity();
        assert!(matches!(
            XmlStream::reserve_bytes(&mut bytes, 1, 0, 4096, &mut usage, 256),
            Err(XmlStreamError::Limit {
                kind: XmlStreamLimit::Work,
                ..
            })
        ));
        assert_eq!(bytes.capacity(), before);
        assert_eq!(usage.retained_bytes, 0);
        usage.work = 0;
        XmlStream::reserve_bytes(&mut bytes, 1, 0, 4096, &mut usage, 257).unwrap();
        assert!(bytes.spilled());
        assert_eq!(usage.retained_bytes, heap_capacity(&bytes));
        assert_eq!(usage.peak_retained_bytes, usage.retained_bytes);
        assert_eq!(usage.work, 257);
    }
    let mut frames = Vec::with_capacity(4);
    frames.resize(
        4,
        Frame {
            name: Span::default(),
            mark: 0,
            bindings: 0,
        },
    );
    let heap = frames.capacity() * size_of::<Frame>();
    let mut usage = XmlStreamUsage {
        retained_bytes: heap,
        peak_retained_bytes: heap,
        ..Default::default()
    };
    let capacity = frames.capacity();
    let copy = frames.len() * size_of::<Frame>();
    assert!(matches!(
        XmlStream::reserve_vec(&mut frames, heap, 4096, &mut usage, copy - 1),
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::Work,
            ..
        })
    ));
    assert_eq!(frames.capacity(), capacity);
    assert_eq!(usage.retained_bytes, heap);
    usage.work = 0;
    XmlStream::reserve_vec(&mut frames, heap, 4096, &mut usage, copy).unwrap();
    assert_eq!(usage.retained_bytes, frames.capacity() * size_of::<Frame>());
    assert_eq!(usage.peak_retained_bytes, usage.retained_bytes);
    // A later sink failure must still report the allocation which succeeded.
    struct Reject;
    impl XmlStreamSink for Reject {
        fn value(&mut self, _: Option<&[u8]>, _: &[u8]) -> Result<(), XmlStreamSinkError> {
            Err(XmlStreamSinkError)
        }
    }
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    parser.feed(XmlStreamInput::Data(b"<r>"), &mut sink).unwrap();
    parser.feed(XmlStreamInput::Data(&vec![b'x'; 1025]), &mut sink).unwrap();
    assert!(parser.content.spilled());
    assert_eq!(
        parser.feed(XmlStreamInput::Data(b"</r>"), &mut Reject),
        Err(XmlStreamError::SinkRejected)
    );
    assert_eq!(parser.usage().retained_bytes, parser.heap().unwrap());
    assert!(parser.usage().peak_retained_bytes >= parser.usage().retained_bytes);
}

#[test]
fn structural_name_admission_precedes_inline_spill_and_excludes_fixed_markers() {
    for limit in [256, 257] {
        let mut parser = XmlStream::new(XmlStreamLimits {
            max_structure_bytes: limit,
            ..XmlStreamLimits::default()
        });
        let mut sink = Sink::default();
        parser.feed(XmlStreamInput::Data(b"<"), &mut sink).unwrap();
        parser.feed(XmlStreamInput::Data(&[b'a'; 256]), &mut sink).unwrap();
        assert_eq!(parser.usage().structure_bytes, 256);
        let heap = parser.heap().unwrap();
        let capacity = parser.name.capacity();
        let result = parser.feed(XmlStreamInput::Data(b"a"), &mut sink);
        if limit == 256 {
            assert_eq!(
                result,
                Err(XmlStreamError::Limit {
                    kind: XmlStreamLimit::StructureBytes,
                    limit
                })
            );
            assert_eq!(parser.name.capacity(), capacity);
            assert!(!parser.name.spilled());
            assert_eq!(parser.heap().unwrap(), heap);
            assert_eq!(parser.usage().retained_bytes, heap);
            assert_eq!(parser.usage().structure_bytes, 256);
        } else {
            result.unwrap();
            assert!(parser.name.spilled());
            assert_eq!(parser.usage().structure_bytes, 257);
            assert_eq!(parser.usage().retained_bytes, parser.heap().unwrap());
        }
    }
    for input in [b"<r><!--x--></r>".as_slice(), b"<r><![CDATA[x]]></r>"] {
        for chunk in 1..=input.len() {
            let mut parser = XmlStream::new(XmlStreamLimits {
                max_name_bytes: 1,
                max_structure_bytes: 2,
                ..XmlStreamLimits::default()
            });
            let mut sink = Sink::default();
            for part in input.chunks(chunk) {
                parser.feed(XmlStreamInput::Data(part), &mut sink).unwrap();
            }
            parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
            assert_eq!(parser.usage().peak_structure_bytes, 2);
        }
    }
}

#[test]
fn spare_capacity_token_bytes_keep_exact_work_and_actual_heap_on_failure() {
    for length in [0, 1023, 1024, 1025, 2047, 2048, 2049] {
        let mut parser = XmlStream::new(XmlStreamLimits::default());
        for _ in 0..length {
            parser.token_byte(b'x').unwrap();
        }
        let before = parser.usage();
        let old_capacity = parser.token.capacity();
        let copy = if length == old_capacity { length } else { 0 };
        let exact = copy + 1;
        parser.limits.max_work = before.work + exact - 1;
        assert!(matches!(
            parser.token_byte(b'y'),
            Err(XmlStreamError::Limit {
                kind: XmlStreamLimit::Work,
                ..
            })
        ));
        assert_eq!(parser.token.len(), length);
        assert_eq!(parser.token.capacity(), old_capacity);
        assert_eq!(parser.usage().retained_bytes, parser.heap().unwrap());
        assert_eq!(parser.usage().peak_retained_bytes, before.peak_retained_bytes);
        parser.usage.work = before.work;
        parser.limits.max_work = before.work + exact;
        parser.token_byte(b'y').unwrap();
        assert_eq!(parser.token.len(), length + 1);
        assert_eq!(parser.usage().work, before.work + exact);
        assert_eq!(parser.usage().retained_bytes, parser.heap().unwrap());
    }
}

#[test]
fn pi_name_release_refreshes_structure_without_relying_on_token_append() {
    for (first, last, peak) in [
        (b"<?xml ".as_slice(), b"version='1.0'?><r/>".as_slice(), 3),
        (b"<?p ".as_slice(), b"data?><r/>".as_slice(), 1),
    ] {
        let mut parser = XmlStream::new(XmlStreamLimits::default());
        let mut sink = Sink::default();
        parser.feed(XmlStreamInput::Data(first), &mut sink).unwrap();
        assert!(parser.name.is_empty());
        assert_eq!(parser.usage().structure_bytes, 0);
        assert_eq!(parser.usage().peak_structure_bytes, peak);
        assert_eq!(parser.usage().retained_bytes, parser.heap().unwrap());
        parser.feed(XmlStreamInput::Data(last), &mut sink).unwrap();
        parser.feed(XmlStreamInput::Complete, &mut sink).unwrap();
        assert_eq!(parser.usage().structure_bytes, 0);
    }
}

#[test]
fn prefixed_attribute_resolution_preserves_syntax_before_redundant_work() {
    for (prefix, extra) in [(b"<r p:x='v'/".as_slice(), 5), (b"<r xmlns:x='u' p:x='v'/", 17)] {
        let mut stream = XmlStream::new(XmlStreamLimits::default());
        let mut sink = Sink::default();
        stream.feed(XmlStreamInput::Data(prefix), &mut sink).unwrap();
        let before = stream.usage().work;
        // Source-byte work, element QName, lexical duplicate checks, and the
        // attribute QName/binding scan remain charged before syntax rejection.
        stream.limits.max_work = before + extra;
        let result = stream.feed(XmlStreamInput::Data(b">"), &mut sink);
        assert_eq!(result, Err(XmlStreamError::Syntax));
        assert_eq!(stream.usage().work, before + extra);
        assert_eq!(stream.feed(XmlStreamInput::Data(b"<r/>"), &mut sink), result);
        assert_eq!(stream.feed(XmlStreamInput::Complete, &mut sink), result);

        let mut limited = XmlStream::new(XmlStreamLimits::default());
        limited.feed(XmlStreamInput::Data(prefix), &mut sink).unwrap();
        limited.limits.max_work = before + extra - 1;
        let error = Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::Work,
            limit: before + extra - 1,
        });
        assert_eq!(limited.feed(XmlStreamInput::Data(b">"), &mut sink), error);
        assert_eq!(limited.feed(XmlStreamInput::Complete, &mut sink), error);
    }
}

#[test]
fn unprefixed_attribute_uniqueness_uses_lexical_names_with_namespace_declarations() {
    for count in [1, 32, 256, 4096, 10_000] {
        for reverse in [false, true] {
            let mut plain = String::from("<r xmlns='urn:default' xmlns:p='urn:p' ");
            let mut qualified = String::from("<r xmlns='urn:default' xmlns:p='urn:p' ");
            for position in 0..count {
                let index = if reverse { count - position - 1 } else { position };
                let spelling = format!("common_prefix_attribute_{index:05}='x' ");
                plain.push_str(&spelling);
                qualified.push_str("p:");
                qualified.push_str(&spelling);
            }
            plain.push_str("/>");
            qualified.push_str("/>");
            let plain = parse(plain.as_bytes(), 16 * 1024).unwrap();
            let qualified = parse(qualified.as_bytes(), 16 * 1024).unwrap();
            assert_eq!(plain.2.value_count, count + 3);
            assert_eq!(plain.2.decoded_bytes + 2 * count, qualified.2.decoded_bytes);
            assert!(plain.1.work < qualified.1.work);
            assert_eq!(plain.0 .0[0].0.as_deref(), Some(b"xmlns".as_slice()));
            assert_eq!(plain.0 .0[1].0.as_deref(), Some(b"xmlns:p".as_slice()));
        }
    }
    for input in [
        b"<r xmlns='u' x='a' x='b'/>".as_slice(),
        b"<r xmlns:p='u' xmlns:p='u'/>",
        b"<r p:x='a' q:x='b' xmlns:p='u' xmlns:q='u'/>",
        b"<r unbound:x='a'/>",
    ] {
        assert_eq!(parse(input, 1).err(), Some(XmlStreamError::Syntax));
    }
}

#[test]
fn qname_delimiters_preserve_first_split_and_validation_error_order() {
    fn original(bytes: &[u8]) -> Result<(), XmlStreamError> {
        for part in bytes.split(|b| *b == b':') {
            validate_ncname(part)?;
        }
        if bytes.iter().filter(|b| **b == b':').count() > 1 {
            return Err(XmlStreamError::Syntax);
        }
        Ok(())
    }
    for bytes in [
        b"".as_slice(),
        b"name",
        b":name",
        b"name:",
        b"a:b",
        b"a:b:c",
        b"a::b",
        "é:ñ".as_bytes(),
    ] {
        let mut stream = XmlStream::new(XmlStreamLimits::default());
        stream.arena.extend_from_slice(b"padding:");
        let span = Span {
            start: stream.arena.len(),
            len: bytes.len(),
        };
        stream.arena.extend_from_slice(bytes);
        stream.arena.extend_from_slice(b":suffix");
        let (prefix, local) = stream.prefix_local(span);
        let split = bytes.iter().position(|b| *b == b':');
        assert_eq!(prefix.start, span.start);
        assert_eq!(prefix.len, split.unwrap_or(0));
        assert_eq!(local.start, span.start + split.map_or(0, |at| at + 1));
        assert_eq!(stream.span(local), &bytes[split.map_or(0, |at| at + 1)..]);
        assert_eq!(validate_qname(bytes), original(bytes));
    }
    assert_eq!(validate_qname(b"a:b:c"), Err(XmlStreamError::Syntax));
    assert_eq!(validate_qname(b"a:b:\xff"), Err(XmlStreamError::Character));
    assert_eq!(validate_qname(b"a::\xff"), Err(XmlStreamError::Syntax));
    let alphabet = [b'a', b':', b'1', 0xff];
    for len in 0..=5 {
        for mut encoded in 0..4_usize.pow(len) {
            let mut bytes = vec![0; len as usize];
            for byte in &mut bytes {
                *byte = alphabet[encoded % 4];
                encoded /= 4;
            }
            assert_eq!(validate_qname(&bytes), original(&bytes), "{bytes:?}");
        }
    }
}

#[test]
fn declaration_scan_preserves_whitespace_quotes_and_attribute_order() {
    fn original(bytes: &[u8]) -> Result<(), XmlStreamError> {
        let mut rest = bytes;
        let mut index = 0;
        while !trim_xml(rest).is_empty() {
            if !rest.first().is_some_and(|b| xml_ws(*b)) {
                return Err(XmlStreamError::Syntax);
            }
            rest = trim_xml(rest);
            let end = rest
                .iter()
                .position(|b| xml_ws(*b) || *b == b'=')
                .ok_or(XmlStreamError::Syntax)?;
            let key = &rest[..end];
            rest = trim_xml(&rest[end..]);
            if rest.first() != Some(&b'=') {
                return Err(XmlStreamError::Syntax);
            }
            rest = trim_xml(&rest[1..]);
            let quote = *rest.first().ok_or(XmlStreamError::Syntax)?;
            if !matches!(quote, b'\'' | b'"') {
                return Err(XmlStreamError::Syntax);
            }
            rest = &rest[1..];
            let end = rest.iter().position(|b| *b == quote).ok_or(XmlStreamError::Syntax)?;
            let value = &rest[..end];
            rest = &rest[end + 1..];
            index = match (index, key, value) {
                (0, b"version", b"1.0") => 1,
                (1, b"encoding", value) if value.eq_ignore_ascii_case(b"UTF-8") => 2,
                (1 | 2, b"standalone", b"yes" | b"no") => 3,
                _ => return Err(XmlStreamError::Syntax),
            };
        }
        if index == 0 {
            Err(XmlStreamError::Syntax)
        } else {
            Ok(())
        }
    }
    for (input, valid) in [
        (b" version='1.0'".as_slice(), true),
        (b"\tversion=\"1.0\" encoding='utf-8' standalone='yes' \r\n", true),
        (b" version='1.0' standalone='no'", true),
        (b"", false),
        (b" \t\r\n", false),
        (b"version='1.0'", false),
        (b" version='1.0'encoding='UTF-8'", false),
        (b" encoding='UTF-8' version='1.0'", false),
        (b" version='1.0' standalone='yes' encoding='UTF-8'", false),
        (b" version='1.0' version='1.0'", false),
        (b" version='1.0", false),
        (b" version='1.0\"", false),
        (b" version='1.0' unknown='yes'", false),
    ] {
        assert_eq!(validate_declaration(input).is_ok(), valid, "{input:?}");
        assert_eq!(validate_declaration(input), original(input));
    }
    for leading in ["", " ", "\t\r\n", "x"] {
        for between in ["", " ", "\n", "  "] {
            for tail in ["", " ", "\t\n", "?"] {
                for first in [
                    "version='1.0'",
                    "version=\"1.0\"",
                    "version='1.1'",
                    "version='",
                    "encoding='UTF-8'",
                ] {
                    for second in [
                        "",
                        "encoding='UTF-8'",
                        "standalone='no'",
                        "standalone=\"yes\"",
                        "version='1.0'",
                        "unknown='x'",
                    ] {
                        let input = format!("{leading}{first}{between}{second}{tail}");
                        assert_eq!(
                            validate_declaration(input.as_bytes()),
                            original(input.as_bytes()),
                            "{input:?}"
                        );
                    }
                }
            }
        }
    }
}
fn text_values(input: &[u8]) -> Vec<Vec<u8>> {
    let expected = parse(input, input.len()).unwrap();
    for chunk in 1..input.len() {
        let result = parse(input, chunk).unwrap();
        assert_eq!(result.0 .0, expected.0 .0, "chunk {chunk}");
        assert_eq!(result.2, expected.2, "chunk {chunk}");
    }
    expected
        .0
         .0
        .into_iter()
        .filter(|(name, _)| name.is_none())
        .map(|(_, value)| value)
        .collect()
}
#[test]
fn element_text_concatenates_across_comments_cdata_and_pis_at_every_split() {
    for (input, expected) in [
        (
            b"<id>1 UNION SEL<!---->ECT password FROM users</id>".as_slice(),
            &[b"1 UNION SELECT password FROM users".as_slice()][..],
        ),
        (
            b"<q>&lt;scr<![CDATA[ipt>alert(1)</scr]]>ipt&gt;</q>",
            &[b"<script>alert(1)</script>".as_slice()],
        ),
        (b"<a>SEL<?pi data?>ECT</a>", &[b"SELECT".as_slice()]),
        // Interior literal whitespace around removed markup is kept.
        (b"<a>SELECT <!-- c --> password</a>", &[b"SELECT  password".as_slice()]),
        // CDATA at both ends is untrimmed; outer literal text is trimmed.
        (b"<a><![CDATA[ x]]> mid <![CDATA[y ]]></a>", &[b" x mid y ".as_slice()]),
        (b"<a>\n x <![CDATA[]]></a>", &[b"x ".as_slice()]),
    ] {
        assert_eq!(text_values(input), expected, "{:?}", String::from_utf8_lossy(input));
    }
}
#[test]
fn root_publishes_its_whole_text_content_once_per_document() {
    for (input, expected) in [
        (b"<a>x<b>y</b>z</a>".as_slice(), &[b"xyz".as_slice()][..]),
        (
            b"<r><s><id>1 UNION SEL<!---->ECT</id></s><n>2</n></r>",
            &[b"1 UNION SELECT2".as_slice()],
        ),
        (b"<a>x<b> y </b>z</a>", &[b"x y z".as_slice()]),
        (b"<a><b/>x<c/></a>", &[b"x".as_slice()]),
        // A payload three elements deep is one value, not one per ancestor.
        (b"<a><b><c>' or 1=1--</c></b></a>", &[b"' or 1=1--".as_slice()]),
    ] {
        assert_eq!(text_values(input), expected, "{:?}", String::from_utf8_lossy(input));
    }
}
#[test]
fn root_without_text_publishes_one_empty_value() {
    // libxml2 xmlNodeGetContent returns an empty string for an element
    // without text, and ModSecurity publishes it, so `&XML:/*` is 1.
    for input in [
        b"<r/>".as_slice(),
        b"<r>  </r>",
        b"<r> <!-- c --> <?pi x?> </r>",
        b"<r><a/>\n<b></b></r>",
        b"<r><![CDATA[]]></r>",
    ] {
        assert_eq!(
            text_values(input),
            [b"".to_vec()],
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
    assert_eq!(text_values(b"<r>\n  <a> </a>\n  <b>v</b>\n</r>"), [b"v".to_vec()]);
}
