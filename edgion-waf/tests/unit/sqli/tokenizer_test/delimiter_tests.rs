use super::*;

#[test]
fn byte_search_returns_first_offset_within_supplied_slice() {
    let tokenizer = SqliTokenizer::new(b"unrelated", SqliFlags::FLAG_NONE);
    for (input, needle, expected) in [
        (b"".as_slice(), 0, None),
        (b"a".as_slice(), b'b', None),
        (b"abca".as_slice(), b'a', Some(0)),
        (b"abca".as_slice(), b'c', Some(2)),
        (b"abc]".as_slice(), b']', Some(3)),
        (b"\0\xff\0".as_slice(), 0, Some(0)),
        (b"\0\xff\0".as_slice(), 0xff, Some(1)),
        (&b"xx'a'"[2..], b'\'', Some(0)),
        (&b"xx'a'"[3..], b'\'', Some(1)),
    ] {
        assert_eq!(tokenizer.memchr(needle, input), expected, "{input:?}, needle {needle}");
    }
}

#[test]
fn adjacent_byte_search_rejects_separated_bytes_and_preserves_first_match() {
    let tokenizer = SqliTokenizer::new(b"unrelated", SqliFlags::FLAG_NONE);
    for (input, first, second, expected) in [
        (b"".as_slice(), b'*', b'/', None),
        (b"*".as_slice(), b'*', b'/', None),
        (b"*x/".as_slice(), b'*', b'/', None),
        (b"*/".as_slice(), b'*', b'/', Some(0)),
        (b"**/".as_slice(), b'*', b'/', Some(1)),
        (b"x*/x*/".as_slice(), b'*', b'/', Some(1)),
        (b"/**/".as_slice(), b'/', b'*', Some(0)),
        (b"aaa".as_slice(), b'a', b'a', Some(0)),
        (b"x\0\xff\0\xff".as_slice(), 0, 0xff, Some(1)),
        (&b"xx*/tail"[2..], b'*', b'/', Some(0)),
        (b"tail*".as_slice(), b'*', b'/', None),
    ] {
        assert_eq!(
            tokenizer.memchr2(input, first, second),
            expected,
            "{input:?}, pair {first}/{second}"
        );
    }
}

#[test]
fn qstring_delimiter_boundary_uses_unsigned_bytes() {
    for delimiter in [b'!', 0x7f] {
        let input = [b'q', b'\'', delimiter, b'x', delimiter, b'\''];
        let mut tokenizer = SqliTokenizer::new(&input, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String, "delimiter {delimiter:#x}");
        assert_eq!(token.val[0], b'x');
        assert_eq!(token.str_close, b'q');
    }

    let input = b"q' x '";
    let mut tokenizer = SqliTokenizer::new(input, SqliFlags::FLAG_NONE);
    assert_ne!(tokenizer.next_token().unwrap().token_type, TokenType::String);
}

#[test]
fn high_byte_qstring_closes_only_on_utf8_encoded_delimiter() {
    for delimiter in [0x80_u8, 0xbf, 0xff] {
        let raw_close = [b'q', b'\'', delimiter, b'x', delimiter, b'\''];
        let mut tokenizer = SqliTokenizer::new(&raw_close, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String);
        assert_eq!(token.str_close, CHAR_NULL);
        assert_eq!(tokenizer.pos, raw_close.len());

        let utf8_close = [
            b'q',
            b'\'',
            delimiter,
            b'x',
            0xc0 | (delimiter >> 6),
            0x80 | (delimiter & 0x3f),
            b'\'',
            b'Z',
        ];
        let mut tokenizer = SqliTokenizer::new(&utf8_close, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String);
        assert_eq!(token.str_close, b'q');
        assert_eq!(tokenizer.pos, utf8_close.len() - 2);
        assert_eq!(tokenizer.next_token().unwrap().token_type, TokenType::String);

        let nq_close = [
            b'n',
            b'q',
            b'\'',
            delimiter,
            b'x',
            0xc0 | (delimiter >> 6),
            0x80 | (delimiter & 0x3f),
            b'\'',
            b'Z',
        ];
        let mut tokenizer = SqliTokenizer::new(&nq_close, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String);
        assert_eq!(token.str_close, b'q');
        assert_eq!(tokenizer.pos, nq_close.len() - 2);
    }
}
