use super::*;

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

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

fn ava(tag: u8, attr: &str, value: &[u8]) -> Vec<u8> {
    tlv(tag, &cat(&[tlv(0x04, attr.as_bytes()), tlv(0x04, value)]))
}

fn encode(text: &str, username: &[u8]) -> Vec<u8> {
    let filter = SearchFilter::compile(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
    let mut out = Vec::new();
    filter.encode(username, &mut out);
    assert_eq!(out.len(), filter.encoded_len(username.len()), "{text}");
    out
}

#[test]
fn equality_exact_bytes() {
    assert_eq!(
        encode("(uid={username})", b"bob"),
        [0xa3, 0x0a, 0x04, 0x03, b'u', b'i', b'd', 0x04, 0x03, b'b', b'o', b'b']
    );
}

#[test]
fn every_filter_kind_exact_bytes() {
    let u = b"bob";
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "(&(objectClass=person)(uid={username}))",
            tlv(0xa0, &cat(&[ava(0xa3, "objectClass", b"person"), ava(0xa3, "uid", u)])),
        ),
        (
            "(|(a={username})(b={username}))",
            tlv(0xa1, &cat(&[ava(0xa3, "a", u), ava(0xa3, "b", u)])),
        ),
        ("(!(a={username}))", tlv(0xa2, &ava(0xa3, "a", u))),
        ("(a>={username})", ava(0xa5, "a", u)),
        ("(a<={username})", ava(0xa6, "a", u)),
        ("(a~={username})", ava(0xa8, "a", u)),
        (
            "(&(a=*)(uid={username}))",
            tlv(0xa0, &cat(&[tlv(0x87, b"a"), ava(0xa3, "uid", u)])),
        ),
        (
            "(cn=x*{username}*y)",
            tlv(
                0xa4,
                &cat(&[
                    tlv(0x04, b"cn"),
                    tlv(0x30, &cat(&[tlv(0x80, b"x"), tlv(0x81, u), tlv(0x82, b"y")])),
                ]),
            ),
        ),
        (
            "(cn=*{username}*)",
            tlv(0xa4, &cat(&[tlv(0x04, b"cn"), tlv(0x30, &tlv(0x81, u))])),
        ),
        (
            "(cn={username}*)",
            tlv(0xa4, &cat(&[tlv(0x04, b"cn"), tlv(0x30, &tlv(0x80, u))])),
        ),
        (
            "(a:1.2.3:={username})",
            tlv(0xa9, &cat(&[tlv(0x81, b"1.2.3"), tlv(0x82, b"a"), tlv(0x83, u)])),
        ),
        (
            "(a:dn:1.2.3:={username})",
            tlv(
                0xa9,
                &cat(&[
                    tlv(0x81, b"1.2.3"),
                    tlv(0x82, b"a"),
                    tlv(0x83, u),
                    vec![0x84, 0x01, 0xff],
                ]),
            ),
        ),
        (
            "(:1.2.3:={username})",
            tlv(0xa9, &cat(&[tlv(0x81, b"1.2.3"), tlv(0x83, u)])),
        ),
        ("(a:={username})", tlv(0xa9, &cat(&[tlv(0x82, b"a"), tlv(0x83, u)]))),
        (
            "(a:DN:={username})",
            tlv(0xa9, &cat(&[tlv(0x82, b"a"), tlv(0x83, u), vec![0x84, 0x01, 0xff]])),
        ),
        (
            "(:dn:1.2.3:={username})",
            tlv(0xa9, &cat(&[tlv(0x81, b"1.2.3"), tlv(0x83, u), vec![0x84, 0x01, 0xff]])),
        ),
        (
            "(memberOf:1.2.840.113556.1.4.1941:=cn=g,dc=x)",
            Vec::new(), // no slot: checked separately below
        ),
        ("(a;lang-en={username})", ava(0xa3, "a;lang-en", u)),
        ("(2.5.4.3={username})", ava(0xa3, "2.5.4.3", u)),
    ];
    for (text, expected) in cases {
        if expected.is_empty() {
            assert_eq!(SearchFilter::compile(text), Err(FilterError::NoUsername));
            continue;
        }
        assert_eq!(encode(text, u), expected, "{text}");
    }
}

#[test]
fn escapes_and_placeholder_detection() {
    assert_eq!(encode("(a=\\2a{username})", b"bob"), ava(0xa3, "a", b"*bob"));
    assert_eq!(encode("(a=x\\28\\29{username})", b"b"), ava(0xa3, "a", b"x()b"));
    assert_eq!(
        SearchFilter::compile("(a=\\7busername\\7d)"),
        Err(FilterError::NoUsername)
    );
    assert_eq!(
        SearchFilter::compile("(a=\\zz{username})"),
        Err(FilterError::InvalidEscape)
    );
    assert_eq!(
        SearchFilter::compile("(a=\\2{username})"),
        Err(FilterError::InvalidEscape)
    );
}

#[test]
fn invalid_filters_are_rejected() {
    let cases: &[(&str, FilterError)] = &[
        ("", FilterError::Syntax),
        ("uid={username}", FilterError::Syntax),
        ("(a={username}", FilterError::Syntax),
        ("(a={username})x", FilterError::Syntax),
        ("(&)", FilterError::Syntax),
        ("(|)", FilterError::Syntax),
        ("(!(a={username})(b={username}))", FilterError::Syntax),
        ("(&(a={username})", FilterError::Syntax),
        ("()", FilterError::Syntax),
        ("(a)", FilterError::Syntax),
        ("(a=x**{username})", FilterError::Syntax),
        ("({username}=x)", FilterError::UsernameOutsideValue),
        ("(a{username}=x)", FilterError::UsernameOutsideValue),
        ("(a b={username})", FilterError::InvalidAttribute),
        ("(1a={username})", FilterError::InvalidAttribute),
        ("(a;={username})", FilterError::InvalidAttribute),
        ("(01.2={username})", FilterError::InvalidAttribute),
        ("(a:caseExactMatch:={username})", FilterError::InvalidAttribute),
        ("(:={username})", FilterError::Syntax),
        ("(a::={username})", FilterError::Syntax),
        ("(a:1.23={username})", FilterError::Syntax),
        ("(a:dn::={username})", FilterError::Syntax),
        ("(:dn:={username})", FilterError::Syntax),
        ("(a:dn:1.2:3:={username})", FilterError::Syntax),
        ("(a=b(c{username})", FilterError::InvalidEscape),
        ("(a~=x*{username})", FilterError::InvalidEscape),
        ("(a=b)", FilterError::NoUsername),
        ("(a=*)", FilterError::NoUsername),
    ];
    for (text, expected) in cases {
        assert_eq!(SearchFilter::compile(text), Err(*expected), "{text:?}");
    }
}

#[test]
fn nul_in_filter_text_is_rejected() {
    assert_eq!(
        SearchFilter::compile("(a=x\0{username})"),
        Err(FilterError::InvalidEscape)
    );
}

fn nested(depth: usize) -> String {
    let mut text = String::new();
    for _ in 0..depth {
        text.push_str("(!");
    }
    text.push_str("(a={username})");
    for _ in 0..depth {
        text.push(')');
    }
    text
}

#[test]
fn depth_and_size_limits() {
    // 15 composites + item = depth 16: accepted.
    let deep = SearchFilter::compile(&nested(15)).unwrap();
    assert_eq!(deep.slots(), 1);
    // 16 composites + item = depth 17: rejected.
    assert_eq!(SearchFilter::compile(&nested(16)), Err(FilterError::TooDeep));

    let mut long = String::from("(a={username}");
    while long.len() < MAX_FILTER_LEN {
        long.push('x');
    }
    long.push(')');
    assert_eq!(long.len(), MAX_FILTER_LEN + 1);
    assert_eq!(SearchFilter::compile(&long), Err(FilterError::TooLong));
    long.truncate(MAX_FILTER_LEN - 1);
    long.push(')');
    assert_eq!(long.len(), MAX_FILTER_LEN);
    assert_eq!(SearchFilter::compile(&long).unwrap().slots(), 1);

    let slots = |n: usize| format!("(|{})", "(a={username})".repeat(n));
    assert_eq!(SearchFilter::compile(&slots(8)).unwrap().slots(), 8);
    assert_eq!(SearchFilter::compile(&slots(9)), Err(FilterError::TooManyUsernames));
}

#[test]
fn deep_filter_compiles_and_encodes_on_a_small_stack() {
    let text = nested(15);
    let bytes = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || encode(&text, b"bob"))
        .unwrap()
        .join()
        .unwrap();
    // 15 NOT wrappers (2 header bytes each) around the 10-byte `(a=bob)` item.
    assert_eq!(bytes.len(), 10 + 15 * 2);
}

#[test]
fn username_bytes_are_literal_in_every_filter_kind() {
    let hostile: &[u8] = b"*)(uid=*\\2a\0";
    assert_eq!(encode("(uid={username})", hostile), ava(0xa3, "uid", hostile));
    assert_eq!(
        encode("(cn=x*{username})", hostile),
        tlv(
            0xa4,
            &cat(&[
                tlv(0x04, b"cn"),
                tlv(0x30, &cat(&[tlv(0x80, b"x"), tlv(0x82, hostile)]))
            ])
        )
    );
    assert_eq!(
        encode("(a:1.2.3:={username})", hostile),
        tlv(0xa9, &cat(&[tlv(0x81, b"1.2.3"), tlv(0x82, b"a"), tlv(0x83, hostile)]))
    );
    assert_eq!(encode("(a>={username})", hostile), ava(0xa5, "a", hostile));
}

#[test]
fn long_usernames_use_long_form_lengths() {
    let username = vec![b'u'; 300];
    let multi = encode("(&(objectClass=person)(|(uid={username})(mail={username})))", &username);
    let expected = tlv(
        0xa0,
        &cat(&[
            ava(0xa3, "objectClass", b"person"),
            tlv(0xa1, &cat(&[ava(0xa3, "uid", &username), ava(0xa3, "mail", &username)])),
        ]),
    );
    assert_eq!(multi, expected);
}

#[test]
fn attribute_description_validation() {
    for good in ["uid", "sAMAccountName", "cn", "a-b", "2.5.4.3", "uid;lang-en", "x;a;b"] {
        assert!(is_attribute_description(good), "{good}");
    }
    for bad in ["", "1uid", "u id", "uid;", "-a", "01.2", "1..2", "a_b", "{username}"] {
        assert!(!is_attribute_description(bad), "{bad}");
    }
}

fn group_bytes(attr: &str, any_of: &[&str], all_of: &[&str], nested: bool) -> Vec<u8> {
    let filter = SearchFilter::group_requirement(attr, any_of, all_of, nested).unwrap();
    assert_eq!(filter.slots(), 0);
    let mut out = Vec::new();
    filter.encode(b"username is ignored", &mut out);
    assert_eq!(out.len(), filter.encoded_len(99));
    out
}

#[test]
fn group_requirement_exact_bytes() {
    let eq = |dn: &str| ava(0xa3, "memberOf", dn.as_bytes());
    // Any-of only: &( |(a) ).
    assert_eq!(
        group_bytes("memberOf", &["cn=a"], &[], false),
        tlv(0xa0, &tlv(0xa1, &eq("cn=a")))
    );
    assert_eq!(
        group_bytes("memberOf", &["cn=a", "cn=b"], &[], false),
        tlv(0xa0, &tlv(0xa1, &cat(&[eq("cn=a"), eq("cn=b")])))
    );
    // All-of only: &(a, b).
    assert_eq!(
        group_bytes("memberOf", &[], &["cn=a", "cn=b"], false),
        tlv(0xa0, &cat(&[eq("cn=a"), eq("cn=b")]))
    );
    // Both: &(all…, |(any…)).
    assert_eq!(
        group_bytes("memberOf", &["cn=x", "cn=y"], &["cn=a"], false),
        tlv(0xa0, &cat(&[eq("cn=a"), tlv(0xa1, &cat(&[eq("cn=x"), eq("cn=y")]))]))
    );
    // Nested: extensible match with the in-chain rule and no dnAttributes.
    let nested = tlv(
        0xa9,
        &cat(&[
            tlv(0x81, IN_CHAIN_RULE.as_bytes()),
            tlv(0x82, b"memberOf"),
            tlv(0x83, b"cn=a"),
        ]),
    );
    assert_eq!(group_bytes("memberOf", &[], &["cn=a"], true), tlv(0xa0, &nested));
}

#[test]
fn group_dn_bytes_are_literal_and_long_values_use_long_lengths() {
    let hostile = "cn=x)(memberOf=*)(cn=\\2a";
    assert_eq!(
        group_bytes("memberOf", &[hostile], &[], false),
        tlv(0xa0, &tlv(0xa1, &ava(0xa3, "memberOf", hostile.as_bytes())))
    );
    let long = format!("cn={}", "a".repeat(1021));
    let item = tlv(
        0xa9,
        &cat(&[
            tlv(0x81, IN_CHAIN_RULE.as_bytes()),
            tlv(0x82, b"memberOf"),
            tlv(0x83, long.as_bytes()),
        ]),
    );
    assert_eq!(
        group_bytes("memberOf", &[], &[long.as_str(); 32], true),
        tlv(0xa0, &item.repeat(32))
    );
}

#[test]
fn group_requirement_errors() {
    assert_eq!(
        SearchFilter::group_requirement("member of", &["cn=a"], &[], false),
        Err(FilterError::InvalidAttribute)
    );
    assert_eq!(
        SearchFilter::group_requirement("memberOf", &[], &[], false),
        Err(FilterError::Syntax)
    );
    assert_eq!(
        SearchFilter::group_requirement("memberOf", &["cn=a", ""], &[], false),
        Err(FilterError::Syntax)
    );
}

#[test]
fn group_requirement_is_pre_encoded_once() {
    let filter = SearchFilter::group_requirement("memberOf", &["cn=a", "cn=b"], &["cn=c"], false).unwrap();
    // One pre-encoded op: per-request encoding is a single copy.
    assert!(matches!(filter.ops.as_slice(), [Op::Bytes(_)]), "{:?}", filter.ops);
    let Op::Bytes(bytes) = &filter.ops[0] else {
        unreachable!()
    };
    let mut out = Vec::new();
    filter.encode(b"x", &mut out);
    assert_eq!(out.as_slice(), &bytes[..]);
    assert_eq!(filter.encoded_len(1), bytes.len());
}
