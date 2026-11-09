use super::*;

#[test]
fn length_octets_rules() {
    assert_eq!(length_octets(0x30, 0x05).unwrap(), 0);
    assert_eq!(length_octets(0x30, 0x81).unwrap(), 1);
    assert_eq!(length_octets(0x30, 0x84).unwrap(), 4);
    assert!(matches!(length_octets(0x30, 0x80), Err(Error::InvalidLength)));
    assert!(matches!(length_octets(0x30, 0x85), Err(Error::InvalidLength)));
    assert!(matches!(
        length_octets(0x16, 0x03),
        Err(Error::UnexpectedTag {
            expected: 0x30,
            actual: 0x16
        })
    ));
    // The identifier is checked before the length octet.
    assert!(matches!(
        length_octets(0x16, 0x80),
        Err(Error::UnexpectedTag { actual: 0x16, .. })
    ));
}

#[test]
fn frame_content_len_rules() {
    assert_eq!(frame_content_len(0x05, &[]).unwrap(), 5);
    assert_eq!(frame_content_len(0x84, &[0, 0, 0, 5]).unwrap(), 5);
    assert_eq!(frame_content_len(0x82, &[0x01, 0x00]).unwrap(), 256);
    assert!(matches!(
        frame_content_len(0x83, &[0x0f, 0x42, 0x3e]),
        Err(Error::FrameTooLarge {
            declared: 999_998,
            max: 65_536
        })
    ));
    assert_eq!(frame_content_len(0x83, &[0x01, 0x00, 0x00]).unwrap(), 65_536);
    assert!(matches!(
        frame_content_len(0x83, &[0x01, 0x00, 0x01]),
        Err(Error::FrameTooLarge { declared: 65_537, .. })
    ));
    assert!(matches!(frame_content_len(0x82, &[0x01]), Err(Error::InvalidLength)));
}

#[test]
fn reader_header_and_truncation() {
    assert!(matches!(Reader::new(&[]).read_header(), Err(Error::Truncated)));
    assert!(matches!(
        Reader::new(&[0x04, 0x05, b'a']).read_header(),
        Err(Error::Truncated)
    ));
    assert!(matches!(
        Reader::new(&[0x1f, 0x00]).read_header(),
        Err(Error::UnsupportedTag)
    ));
    assert!(matches!(
        Reader::new(&[0x04, 0x80]).read_header(),
        Err(Error::InvalidLength)
    ));
    assert!(matches!(
        Reader::new(&[0x04, 0x84, 0xff, 0xff, 0xff, 0xff]).read_header(),
        Err(Error::Truncated)
    ));
    assert!(matches!(
        Reader::new(&[0x04, 0x82, 0x01]).read_header(),
        Err(Error::Truncated)
    ));
}

fn integer(content: &[u8]) -> Vec<u8> {
    let mut out = vec![INTEGER, content.len() as u8];
    out.extend_from_slice(content);
    out
}

#[test]
fn read_u32_widths() {
    let ok = [
        (&[0x00][..], 0u32),
        (&[0x7f], 127),
        (&[0x00, 0x80], 128),
        (&[0x01, 0x00], 256),
        (&[0x00, 0x00, 0x05], 5),
        (&[0x00, 0xff, 0xff, 0xff, 0xff], u32::MAX),
    ];
    for (content, expected) in ok {
        assert_eq!(
            Reader::new(&integer(content)).read_u32(INTEGER).unwrap(),
            expected,
            "{content:02x?}"
        );
    }
    let bad: [&[u8]; 4] = [&[], &[0x80], &[0x01, 0x00, 0x00, 0x00, 0x00], &[0, 0, 0, 0, 0, 0]];
    for content in bad {
        assert!(
            matches!(
                Reader::new(&integer(content)).read_u32(INTEGER),
                Err(Error::InvalidInteger)
            ),
            "{content:02x?}"
        );
    }
    assert!(matches!(
        Reader::new(&integer(&[1])).read_u32(ENUMERATED),
        Err(Error::UnexpectedTag {
            expected: 0x0a,
            actual: 0x02
        })
    ));
}

#[test]
fn read_utf8_octets_rules() {
    assert!(Reader::new(&[0x04, 0x03, b'a', b'b', b'c']).read_utf8_octets().is_ok());
    assert!(Reader::new(&[0x04, 0x02, 0xc3, 0xa9]).read_utf8_octets().is_ok());
    assert!(matches!(
        Reader::new(&[0x04, 0x01, 0xff]).read_utf8_octets(),
        Err(Error::InvalidUtf8)
    ));
    assert!(matches!(
        Reader::new(&[0x24, 0x00]).read_utf8_octets(),
        Err(Error::UnexpectedTag {
            expected: 0x04,
            actual: 0x24
        })
    ));
}

#[test]
fn expect_and_skip_stay_inside_their_container() {
    let bytes = [0x30, 0x03, 0x02, 0x01, 0x07, 0x04, 0x00];
    let mut outer = Reader::new(&bytes);
    let mut inner = outer.expect(SEQUENCE).unwrap();
    assert_eq!(inner.read_u32(INTEGER).unwrap(), 7);
    assert!(inner.is_empty());
    assert!(!outer.is_empty());
    outer.skip_element().unwrap();
    assert!(outer.is_empty());
}

#[test]
fn writer_round_trips() {
    for value in [0, 1, 127, 128, 255, 256, 65_535, u32::MAX] {
        let mut out = Vec::new();
        write_u32(&mut out, INTEGER, value);
        assert_eq!(out.len(), element_len(u32_content_len(value)));
        assert_eq!(Reader::new(&out).read_u32(INTEGER).unwrap(), value);
    }
    let mut out = Vec::new();
    write_u32(&mut out, INTEGER, 128);
    assert_eq!(out, [0x02, 0x02, 0x00, 0x80]);

    for len in [0usize, 127, 128, 255, 256, 70_000] {
        let mut out = Vec::new();
        write_header(&mut out, OCTET_STRING, len);
        assert_eq!(out.len(), header_len(len));
        out.resize(out.len() + len, 0);
        assert_eq!(Reader::new(&out).read_header().unwrap(), (OCTET_STRING, len));
    }
    let mut out = Vec::new();
    write_header(&mut out, OCTET_STRING, 128);
    assert_eq!(out, [0x04, 0x81, 0x80]);
    let mut out = Vec::new();
    write_header(&mut out, OCTET_STRING, 256);
    assert_eq!(out, [0x04, 0x82, 0x01, 0x00]);
}
