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
