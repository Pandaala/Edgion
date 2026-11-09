use super::*;
use crate::codec::{frame_content_len, length_octets};

/// Run a complete frame through the same header path `BindSession` uses.
fn decode_frame(frame: &[u8], id: u32) -> Result<LdapResult, Error> {
    let (&tag, rest) = frame.split_first().ok_or(Error::Truncated)?;
    let (&first, rest) = rest.split_first().ok_or(Error::Truncated)?;
    let extra = length_octets(tag, first)?;
    let octets = rest.get(..extra).ok_or(Error::Truncated)?;
    let len = frame_content_len(first, octets)?;
    let content = rest.get(extra..extra + len).ok_or(Error::Truncated)?;
    decode_bind_response(content, id)
}

/// `depth` nested `30 8n <n length octets>` headers around `02 01 01`,
/// built iteratively.
fn nest(depth: usize, length_octets: usize) -> Vec<u8> {
    let mut inner = vec![0x02, 0x01, 0x01];
    for _ in 0..depth {
        let len = inner.len();
        let mut outer = Vec::with_capacity(len + 2 + length_octets);
        outer.push(0x30);
        outer.push(0x80 | length_octets as u8);
        for index in (0..length_octets).rev() {
            outer.push((len >> (8 * index)) as u8);
        }
        outer.extend_from_slice(&inner);
        inner = outer;
    }
    inner
}

/// The auth-02 shape: `depth` nested `30 83 L2 L1 L0` headers.
fn flood(depth: usize) -> Vec<u8> {
    nest(depth, 3)
}

fn declared_len(frame: &[u8]) -> usize {
    frame[2..5].iter().fold(0usize, |acc, &b| (acc << 8) | usize::from(b))
}

#[test]
fn auth01_vectors_and_floods_return_exact_errors() {
    assert!(matches!(
        decode_frame(&[0x30, 0x05, 0x02, 0x01, 0x01, 0x61, 0x00], 1),
        Err(Error::Truncated)
    ));
    assert!(matches!(
        decode_frame(
            &[0x30, 0x0d, 0x02, 0x01, 0x01, 0x61, 0x08, 0x0a, 0x01, 0x00, 0x04, 0x01, 0xff, 0x04, 0x00],
            1
        ),
        Err(Error::InvalidUtf8)
    ));
    assert!(matches!(
        decode_frame(&[0x30, 0x05, 0x02, 0x01, 0x01, 0x41, 0x00], 1),
        Err(Error::UnexpectedTag {
            expected: 0x61,
            actual: 0x41
        })
    ));
    assert!(matches!(decode_frame(&[0x30, 0x00], 1), Err(Error::Truncated)));

    let shallow = flood(64);
    assert_eq!(declared_len(&shallow), 318);
    assert!(matches!(
        decode_frame(&shallow, 1),
        Err(Error::UnexpectedTag {
            expected: 0x02,
            actual: 0x30
        })
    ));

    let deep = flood(200_000);
    assert_eq!(declared_len(&deep), 999_998);
    assert!(matches!(
        decode_frame(&deep, 1),
        Err(Error::FrameTooLarge {
            declared: 999_998,
            max: 65_536
        })
    ));
}

#[cfg(feature = "client")]
#[test]
fn bind_request_bytes_exact() {
    assert_eq!(
        encode_bind_request(1, "cn=a", b"pw"),
        [
            0x30, 0x12, 0x02, 0x01, 0x01, 0x60, 0x0d, 0x02, 0x01, 0x03, 0x04, 0x04, b'c', b'n', b'=', b'a', 0x80, 0x02,
            b'p', b'w'
        ]
    );
}

#[cfg(feature = "client")]
#[test]
fn long_bind_dn_uses_long_form_lengths() {
    let dn = "a".repeat(300);
    let frame = encode_bind_request(2, &dn, b"secret");
    let mut reader = Reader::new(&frame);
    let mut message = reader.expect(SEQUENCE).unwrap();
    assert!(reader.is_empty());
    assert_eq!(message.read_u32(INTEGER).unwrap(), 2);
    let mut op = message.expect(BIND_REQUEST).unwrap();
    assert_eq!(op.read_u32(INTEGER).unwrap(), 3);
    op.read_utf8_octets().unwrap();
    op.skip_element().unwrap();
    assert!(op.is_empty() && message.is_empty());
}

#[cfg(feature = "client")]
#[test]
fn unbind_and_start_tls_bytes_exact() {
    assert_eq!(encode_unbind_request(3), [0x30, 0x05, 0x02, 0x01, 0x03, 0x42, 0x00]);
    let mut expected = vec![0x30, 0x1d, 0x02, 0x01, 0x01, 0x77, 0x18, 0x80, 0x16];
    expected.extend_from_slice(b"1.3.6.1.4.1.1466.20037");
    assert_eq!(encode_start_tls_request(1), expected);
}

fn bind_response(id: u8, rc: &[u8], tail: &[u8], after_op: &[u8]) -> Vec<u8> {
    let mut op = vec![0x0a, rc.len() as u8];
    op.extend_from_slice(rc);
    op.extend_from_slice(&[0x04, 0x00, 0x04, 0x00]);
    op.extend_from_slice(tail);
    let mut content = vec![0x02, 0x01, id, 0x61];
    assert!(op.len() < 0x80);
    content.push(op.len() as u8);
    content.extend_from_slice(&op);
    content.extend_from_slice(after_op);
    content
}

#[test]
fn bind_response_result_codes() {
    for (rc, expected) in [(&[0x00][..], 0), (&[0x31], 49), (&[0x35], 53), (&[0x00, 0x31], 49)] {
        let content = bind_response(1, rc, &[], &[]);
        assert_eq!(decode_bind_response(&content, 1).unwrap().result_code, expected);
    }
}

#[test]
fn tails_are_skipped() {
    let referral = [0xa3, 0x06, 0x04, 0x04, b'l', b'd', b'a', b'p'];
    let content = bind_response(1, &[0x0a], &referral, &[0xa0, 0x00, 0x8a, 0x00]);
    assert_eq!(decode_bind_response(&content, 1).unwrap().result_code, 10);

    let overrun = bind_response(1, &[0x00], &[], &[0xa0, 0x05, 0x00]);
    assert!(matches!(decode_bind_response(&overrun, 1), Err(Error::Truncated)));

    let nested = bind_response(1, &[0x00], &[], &flood(64));
    assert_eq!(decode_bind_response(&nested, 1).unwrap().result_code, 0);

    // A truncated referral inside the BindResponse itself.
    let inner_overrun = bind_response(1, &[0x00], &[0xa3, 0x05, 0x04], &[]);
    assert!(matches!(decode_bind_response(&inner_overrun, 1), Err(Error::Truncated)));
}

/// Guards the no-recursion rule inside the frame cap: a 16 000-level nest fits
/// in one 64 KiB frame and is skipped by length. The decode runs on a 128 KiB
/// stack, so a decoder that descends into constructed elements recursively
/// overflows it in debug and release builds alike.
#[test]
fn deep_nesting_within_frame_cap_is_skipped_without_descent() {
    let tail = nest(16_000, 2);
    let content = bind_response(1, &[0x00], &[], &tail);
    assert!(content.len() <= crate::codec::MAX_FRAME);
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || decode_bind_response(&content, 1).map(|result| result.result_code))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn message_id_checks() {
    for actual in [0u8, 7] {
        let content = bind_response(actual, &[0x00], &[], &[]);
        assert!(matches!(
            decode_bind_response(&content, 1),
            Err(Error::UnexpectedMessageId { expected: 1, actual: a }) if a == u32::from(actual)
        ));
    }
}

#[test]
fn extended_response_tag_and_name() {
    let mut op = vec![0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00, 0x8a, 0x16];
    op.extend_from_slice(b"1.3.6.1.4.1.1466.20037");
    let mut content = vec![0x02, 0x01, 0x01, 0x78, op.len() as u8];
    content.extend_from_slice(&op);
    assert_eq!(decode_extended_response(&content, 1).unwrap().result_code, 0);

    let bind = bind_response(1, &[0x00], &[], &[]);
    assert!(matches!(
        decode_extended_response(&bind, 1),
        Err(Error::UnexpectedTag {
            expected: 0x78,
            actual: 0x61
        })
    ));
}

#[test]
fn diagnostic_message_must_be_utf8() {
    let content = [
        0x02, 0x01, 0x01, 0x61, 0x09, 0x0a, 0x01, 0x31, 0x04, 0x01, b'x', 0x04, 0x01, 0xff,
    ];
    assert!(matches!(decode_bind_response(&content, 1), Err(Error::InvalidUtf8)));
}

// ── Search mode ─────────────────────────────────────────────────────────

/// Independent minimal-length BER TLV builder for expected values.
fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(content);
    out
}

fn partial(attribute_type: &str, values: &[&[u8]]) -> Vec<u8> {
    let vals: Vec<u8> = values.iter().flat_map(|value| tlv(0x04, value)).collect();
    tlv(0x30, &[tlv(0x04, attribute_type.as_bytes()), tlv(0x31, &vals)].concat())
}

/// Content (after the outer SEQUENCE) of a SearchResultEntry message.
fn entry_content(id: u8, dn: &[u8], attributes: &[Vec<u8>]) -> Vec<u8> {
    let entry = [tlv(0x04, dn), tlv(0x30, &attributes.concat())].concat();
    [tlv(0x02, &[id]), tlv(0x64, &entry)].concat()
}

fn done_content(id: u8, rc: u8) -> Vec<u8> {
    [tlv(0x02, &[id]), tlv(0x65, &[0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00])].concat()
}

#[cfg(feature = "client")]
#[test]
fn search_request_bytes_exact() {
    let filter = crate::filter::SearchFilter::compile("(uid={username})").unwrap();
    let op = [
        tlv(0x04, b"dc=x"),
        tlv(0x0a, &[2]),
        tlv(0x0a, &[0]),
        tlv(0x02, &[2]),
        tlv(0x02, &[5]),
        vec![0x01, 0x01, 0x00],
        vec![0xa3, 0x0a, 0x04, 0x03, b'u', b'i', b'd', 0x04, 0x03, b'b', b'o', b'b'],
        tlv(0x30, &tlv(0x04, b"uid")),
    ]
    .concat();
    let expected = tlv(0x30, &[tlv(0x02, &[3]), tlv(0x63, &op)].concat());
    assert_eq!(
        encode_search_request(3, "dc=x", Scope::Subtree, &filter, b"bob", "uid", 5),
        expected
    );
}

#[cfg(feature = "client")]
#[test]
fn search_request_with_long_filter_and_large_time_limit() {
    let filter = crate::filter::SearchFilter::compile("(|(uid={username})(mail={username}))").unwrap();
    let username = vec![b'x'; 200];
    let bytes = encode_search_request(
        2,
        "dc=example,dc=org",
        Scope::Subtree,
        &filter,
        &username,
        "uid",
        i32::MAX as u32,
    );
    let mut reader = Reader::new(&bytes);
    let mut message = reader.expect(SEQUENCE).unwrap();
    assert!(reader.is_empty());
    assert_eq!(message.read_u32(INTEGER).unwrap(), 2);
    let mut op = message.expect(0x63).unwrap();
    op.skip_element().unwrap(); // base
    assert_eq!(op.read_u32(ENUMERATED).unwrap(), 2);
    assert_eq!(op.read_u32(ENUMERATED).unwrap(), 0);
    assert_eq!(op.read_u32(INTEGER).unwrap(), 2);
    assert_eq!(op.read_u32(INTEGER).unwrap(), i32::MAX as u32);
    op.skip_element().unwrap(); // typesOnly
    op.skip_element().unwrap(); // filter
    op.skip_element().unwrap(); // attributes
    assert!(op.is_empty() && message.is_empty());
}

#[test]
fn search_entry_with_identity() {
    let content = entry_content(
        2,
        b"uid=bob,ou=people,dc=x",
        &[partial("cn", &[b"Bob"]), partial("uid", &[b"bob"])],
    );
    assert_eq!(
        decode_search_frame(&content, 2, "uid").unwrap(),
        SearchFrame::Entry {
            dn: "uid=bob,ou=people,dc=x".to_string(),
            identity: Some("bob".to_string())
        }
    );
}

#[test]
fn entry_identity_type_case_insensitive() {
    let content = entry_content(2, b"uid=bob,dc=x", &[partial("UID", &[b"bob"])]);
    let SearchFrame::Entry { identity, .. } = decode_search_frame(&content, 2, "uid").unwrap() else {
        panic!("entry expected")
    };
    assert_eq!(identity.as_deref(), Some("bob"));
}

#[test]
fn entry_identity_keeps_non_ascii_whitespace() {
    // Only SP is trimmed by HTTP field parsing; other Unicode spaces stay valid.
    let value = "yamada\u{3000}";
    let content = entry_content(2, b"uid=y,dc=x", &[partial("uid", &[value.as_bytes()])]);
    let SearchFrame::Entry { identity, .. } = decode_search_frame(&content, 2, "uid").unwrap() else {
        panic!("entry expected")
    };
    assert_eq!(identity.as_deref(), Some(value));
}

#[test]
fn entry_identity_rejections() {
    let cases: Vec<Vec<Vec<u8>>> = vec![
        vec![],                                                   // attribute absent
        vec![partial("uid;lang-en", &[b"bob"])],                  // option-bearing type
        vec![partial("uid", &[b"bob", b"bobby"])],                // two values
        vec![partial("uid", &[b"bob"]), partial("uid", &[b"x"])], // two elements
        vec![partial("uid", &[b""])],                             // empty value
        vec![partial("uid", &[b"bo\x01b"])],                      // control character
        vec![partial("uid", &[b"bo\tb"])],                        // TAB
        vec![partial("uid", &[b" admin"])],                       // leading space
        vec![partial("uid", &[b"admin "])],                       // trailing space
        vec![partial("uid", &[&[0xff, 0xfe]])],                   // not UTF-8
        vec![partial("uid", &[])],                                // no values
    ];
    for attributes in cases {
        let content = entry_content(2, b"uid=bob,dc=x", &attributes);
        let SearchFrame::Entry { identity, .. } = decode_search_frame(&content, 2, "uid").unwrap() else {
            panic!("entry expected")
        };
        assert_eq!(identity, None, "{attributes:02x?}");
    }
}

#[test]
fn search_frame_kinds_and_errors() {
    assert_eq!(
        decode_search_frame(&done_content(2, 0), 2, "uid").unwrap(),
        SearchFrame::Done(LdapResult { result_code: 0 })
    );
    assert_eq!(
        decode_search_frame(&done_content(2, 4), 2, "uid").unwrap(),
        SearchFrame::Done(LdapResult { result_code: 4 })
    );
    let reference = [tlv(0x02, &[2]), tlv(0x73, &tlv(0x04, b"ldap://other/dc=x"))].concat();
    assert_eq!(
        decode_search_frame(&reference, 2, "uid").unwrap(),
        SearchFrame::Reference
    );
    assert!(matches!(
        decode_search_frame(&done_content(3, 0), 2, "uid"),
        Err(Error::UnexpectedMessageId { expected: 2, actual: 3 })
    ));
    let bind = [tlv(0x02, &[2]), tlv(0x61, &[0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00])].concat();
    assert!(matches!(
        decode_search_frame(&bind, 2, "uid"),
        Err(Error::UnexpectedTag {
            expected: 0x65,
            actual: 0x61
        })
    ));
    let bad_dn = entry_content(2, &[0xff], &[]);
    assert!(matches!(
        decode_search_frame(&bad_dn, 2, "uid"),
        Err(Error::InvalidUtf8)
    ));
    let truncated = [tlv(0x02, &[2]), vec![0x64, 0x05, 0x04, 0x01]].concat();
    assert!(decode_search_frame(&truncated, 2, "uid").is_err());
}

#[test]
fn deeply_nested_attribute_value_is_rejected_without_recursion() {
    // A 16 000-level nest sits inside a value SET element of an unrelated attribute.
    let nest = nest(16_000, 2);
    let mut odd = tlv(0x04, b"description");
    odd.extend_from_slice(&tlv(0x31, &nest));
    let attributes = vec![tlv(0x30, &odd), partial("uid", &[b"bob"])];
    let content = entry_content(2, b"uid=bob,dc=x", &attributes);
    assert!(content.len() <= crate::codec::MAX_FRAME);
    let result = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || decode_search_frame(&content, 2, "uid").map(|frame| matches!(frame, SearchFrame::Entry { .. })))
        .unwrap()
        .join()
        .unwrap();
    // The nested SEQUENCE inside the SET is not an OCTET STRING value: a
    // structural error, never a stack overflow.
    assert!(matches!(
        result,
        Err(Error::UnexpectedTag {
            expected: 0x04,
            actual: 0x30
        })
    ));
}

#[cfg(feature = "client")]
#[test]
fn base_scope_search_request_bytes_exact() {
    let filter = crate::filter::SearchFilter::group_requirement("memberOf", &["cn=g"], &[], false).unwrap();
    let group_filter = tlv(
        0xa0,
        &tlv(0xa1, &tlv(0xa3, &[tlv(0x04, b"memberOf"), tlv(0x04, b"cn=g")].concat())),
    );
    let op = [
        tlv(0x04, b"uid=bob,dc=x"),
        tlv(0x0a, &[0]),
        tlv(0x0a, &[0]),
        tlv(0x02, &[1]),
        tlv(0x02, &[5]),
        vec![0x01, 0x01, 0x00],
        group_filter,
        tlv(0x30, &tlv(0x04, b"1.1")),
    ]
    .concat();
    let expected = tlv(0x30, &[tlv(0x02, &[4]), tlv(0x63, &op)].concat());
    // The username is ignored by a filter without a slot.
    assert_eq!(
        encode_search_request(4, "uid=bob,dc=x", Scope::Base, &filter, b"ignored", "1.1", 5),
        expected
    );
}
