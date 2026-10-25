use super::*;

#[test]
fn dollar_dispatch_preserves_all_byte_classifications() {
    for byte in 0..=u8::MAX {
        let input = [b'$', byte, b'$', b'x', b'$', byte, b'$'];
        let (kind, start, content, cursor, quote) = match byte {
            b'A'..=b'Z' | b'a'..=b'z' => (TokenType::String, 3, b"x".as_slice(), 7, b'$'),
            b'0'..=b'9' | b',' => (TokenType::Number, 0, &input[..2], 2, CHAR_NULL),
            b'.' => (TokenType::Bareword, 0, input.as_slice(), 7, CHAR_NULL),
            b'$' => (TokenType::String, 2, b"$x".as_slice(), 6, b'$'),
            _ => (TokenType::Bareword, 0, b"$".as_slice(), 1, CHAR_NULL),
        };
        let mut tokenizer = SqliTokenizer::new(&input, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, kind, "byte {byte}");
        assert_eq!(
            (token.pos, token.len, token.count),
            (start, content.len(), 0),
            "byte {byte}"
        );
        let mut expected = [0; 32];
        expected[..content.len()].copy_from_slice(content);
        assert_eq!(token.val, expected, "byte {byte}");
        assert_eq!((token.str_open, token.str_close), (quote, quote), "byte {byte}");
        assert_eq!(tokenizer.pos, cursor, "byte {byte}");
    }
}

#[test]
fn money_sequences_and_nonletter_tag_suffixes_keep_dispatch_order() {
    for (input, kind, content, cursor) in [
        (b"$1,234.50!".as_slice(), TokenType::Number, b"$1,234.50".as_slice(), 9),
        (b"$.,!".as_slice(), TokenType::Number, b"$.,".as_slice(), 3),
        (b"$.!".as_slice(), TokenType::Bareword, b"$.".as_slice(), 2),
        (b"$Ab9$x$Ab9$".as_slice(), TokenType::Bareword, b"$".as_slice(), 1),
        (b"$Ab_x$".as_slice(), TokenType::Bareword, b"$".as_slice(), 1),
        (b"$Ab\xff$x$".as_slice(), TokenType::Bareword, b"$".as_slice(), 1),
    ] {
        let mut tokenizer = SqliTokenizer::new(input, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, kind, "{input:?}");
        assert_eq!((token.pos, token.len, token.count), (0, content.len(), 0));
        let mut expected = [0; 32];
        expected[..content.len()].copy_from_slice(content);
        assert_eq!(token.val, expected);
        assert_eq!((token.str_open, token.str_close), (CHAR_NULL, CHAR_NULL));
        assert_eq!(tokenizer.pos, cursor);
    }
}

#[test]
fn line_comments_keep_lf_only_boundary_and_dialect_statistics() {
    for (input, start, content, cursor, flags, stats) in [
        (
            b"-- a\r\n!".as_slice(),
            0,
            b"-- a\r".as_slice(),
            6,
            SqliFlags::FLAG_SQL_ANSI,
            (1, 0, 0),
        ),
        (
            b"--x\n!".as_slice(),
            0,
            b"--x".as_slice(),
            4,
            SqliFlags::FLAG_SQL_ANSI,
            (0, 1, 0),
        ),
        (
            b"#a\r\n!".as_slice(),
            0,
            b"#a\r".as_slice(),
            4,
            SqliFlags::FLAG_SQL_MYSQL,
            (0, 0, 2),
        ),
        (
            b"-- \0\xff\n!".as_slice(),
            0,
            b"-- \0\xff".as_slice(),
            6,
            SqliFlags::FLAG_SQL_ANSI,
            (1, 0, 0),
        ),
        (
            b" -- a\n!".as_slice(),
            1,
            b"-- a".as_slice(),
            6,
            SqliFlags::FLAG_SQL_ANSI,
            (1, 0, 0),
        ),
        (
            b"-- a\r".as_slice(),
            0,
            b"-- a\r".as_slice(),
            5,
            SqliFlags::FLAG_SQL_ANSI,
            (1, 0, 0),
        ),
        (
            b"--\n".as_slice(),
            0,
            b"--".as_slice(),
            3,
            SqliFlags::FLAG_SQL_ANSI,
            (1, 0, 0),
        ),
    ] {
        let mut tokenizer = SqliTokenizer::new(input, flags);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::Comment);
        assert_eq!((token.pos, token.len, token.count), (start, content.len(), 0));
        let mut expected = [0; 32];
        expected[..content.len()].copy_from_slice(content);
        assert_eq!(token.val, expected);
        assert_eq!((token.str_open, token.str_close), (CHAR_NULL, CHAR_NULL));
        assert_eq!(tokenizer.pos, cursor);
        assert_eq!(
            (
                tokenizer.stats_comment_ddw,
                tokenizer.stats_comment_ddx,
                tokenizer.stats_comment_hash
            ),
            stats
        );
        if cursor < input.len() {
            let next = tokenizer.next_token().unwrap();
            assert_eq!((next.pos, next.len, next.val[0]), (cursor, 1, b'!'));
        }
        assert!(tokenizer.next_token().is_none());
    }

    let mut input = b"-- ".to_vec();
    input.extend_from_slice(&[b'a'; 64]);
    input.push(b'\n');
    let mut tokenizer = SqliTokenizer::new(&input, SqliFlags::FLAG_SQL_ANSI);
    let token = tokenizer.next_token().unwrap();
    assert_eq!((token.pos, token.len, token.count), (0, 31, 0));
    let mut expected = [b'a'; 32];
    expected[..3].copy_from_slice(b"-- ");
    expected[31] = CHAR_NULL;
    assert_eq!(token.val, expected);
    assert_eq!(tokenizer.pos, 68);
    assert!(tokenizer.next_token().is_none());
}

#[test]
fn qstring_search_keeps_encoded_delimiter_and_absolute_first_match() {
    for (input, start, delimiter, expected) in [
        (b"".as_slice(), 0, b'!', None),
        (b"!".as_slice(), 0, b'!', None),
        (b"!!'".as_slice(), 0, b'!', Some(1)),
        (b"xx!x!'tail!'".as_slice(), 2, b'!', Some(4)),
        (b"!'".as_slice(), 1, b'!', None),
        (b"!'".as_slice(), 2, b'!', None),
        (b"!'".as_slice(), 3, b'!', None),
        (b"\xff'\xc3\xbf'\xc3\xbf'".as_slice(), 0, 0xff, Some(2)),
        (b"\xff'\xc3\xbf'\xc3\xbf'".as_slice(), 3, 0xff, Some(5)),
        (b"\xc3\xbf".as_slice(), 0, 0xff, None),
        (b"\xc3\xbfX'".as_slice(), 0, 0xff, None),
        (b"\0\xc2\x80'".as_slice(), 0, 0x80, Some(1)),
        (b"\xc2\x80'".as_slice(), 3, 0x80, None),
    ] {
        let tokenizer = SqliTokenizer::new(input, SqliFlags::FLAG_NONE);
        assert_eq!(
            tokenizer.find_qstring_end(start, delimiter),
            expected,
            "{input:?}, start {start}"
        );
    }
}

#[test]
fn dollar_strings_preserve_first_exact_end_marker_and_cursor() {
    for (input, start, content, cursor, closed) in [
        (b"$$".as_slice(), 2, b"".as_slice(), 2, false),
        (b"$$$".as_slice(), 2, b"$".as_slice(), 3, false),
        (b"$$$$!".as_slice(), 2, b"".as_slice(), 4, true),
        (b"$$a$$!".as_slice(), 2, b"a".as_slice(), 5, true),
        (b"$$a$".as_slice(), 2, b"a$".as_slice(), 4, false),
        (b"$$a$$$$".as_slice(), 2, b"a".as_slice(), 5, true),
        (b" $$x$$!".as_slice(), 3, b"x".as_slice(), 6, true),
        (b"$x$".as_slice(), 3, b"".as_slice(), 3, false),
        (b"$x$$x$!".as_slice(), 3, b"".as_slice(), 6, true),
        (b"$Ab$x$ab$y$Ab$!".as_slice(), 4, b"x$ab$y".as_slice(), 14, true),
        (b"$t$x$t".as_slice(), 3, b"x$t".as_slice(), 6, false),
        (b"$t$\0\xff$t$!".as_slice(), 3, b"\0\xff".as_slice(), 8, true),
        (b" $t$x$t$!".as_slice(), 4, b"x".as_slice(), 8, true),
    ] {
        let mut tokenizer = SqliTokenizer::new(input, SqliFlags::FLAG_NONE);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String, "{input:?}");
        assert_eq!(
            (token.pos, token.len, token.count),
            (start, content.len(), 0),
            "{input:?}"
        );
        let mut expected = [0; 32];
        expected[..content.len()].copy_from_slice(content);
        assert_eq!(token.val, expected, "{input:?}");
        assert_eq!(
            (token.str_open, token.str_close),
            (b'$', if closed { b'$' } else { CHAR_NULL })
        );
        assert_eq!(tokenizer.pos, cursor, "{input:?}");
        if input.get(cursor) == Some(&b'!') {
            let next = tokenizer.next_token().unwrap();
            assert_eq!((next.pos, next.len, next.val[0]), (cursor, 1, b'!'));
            assert!(tokenizer.next_token().is_none());
        }
    }
}

#[test]
fn long_dollar_tag_near_match_preserves_truncation_and_full_cursor() {
    let mut tag = vec![b'$'];
    tag.extend_from_slice(&[b'a'; 64]);
    tag.push(b'$');
    let mut near_match = tag.clone();
    near_match[64] = b'b';
    let mut input = tag.clone();
    input.extend_from_slice(&near_match);
    input.extend_from_slice(&tag);
    let mut tokenizer = SqliTokenizer::new(&input, SqliFlags::FLAG_NONE);
    let token = tokenizer.next_token().unwrap();
    assert_eq!(token.token_type, TokenType::String);
    assert_eq!((token.pos, token.len, token.count), (66, 31, 0));
    let mut expected = [b'a'; 32];
    expected[0] = b'$';
    expected[31] = CHAR_NULL;
    assert_eq!(token.val, expected);
    assert_eq!((token.str_open, token.str_close), (b'$', b'$'));
    assert_eq!(tokenizer.pos, 198);
    assert!(tokenizer.next_token().is_none());
}

#[test]
fn simulated_quote_search_preserves_escape_order_and_cursor() {
    for (quote, flags) in [
        (b'\'', SqliFlags::FLAG_QUOTE_SINGLE | SqliFlags::FLAG_SQL_ANSI),
        (b'"', SqliFlags::FLAG_QUOTE_DOUBLE | SqliFlags::FLAG_SQL_MYSQL),
    ] {
        let mut empty = SqliTokenizer::new(b"", flags);
        assert!(empty.next_token().is_none());
        assert_eq!(empty.pos, 0);

        for (source, content_len, cursor, closed) in [
            (b"abc".as_slice(), 3, 3, false),
            (b"'".as_slice(), 0, 1, true),
            (b"abc'".as_slice(), 3, 4, true),
            (br"a\'b'!".as_slice(), 4, 5, true),
            (br"a\\'!".as_slice(), 3, 4, true),
            (b"a''b'!".as_slice(), 4, 5, true),
            (b"'''!".as_slice(), 2, 3, true),
            (br"a\'".as_slice(), 3, 3, false),
            (b"a''".as_slice(), 3, 3, false),
            (br"a\'b''c'!".as_slice(), 7, 8, true),
            (b"\0\xff'!".as_slice(), 2, 3, true),
        ] {
            let input: Vec<_> = source
                .iter()
                .map(|&byte| if byte == b'\'' { quote } else { byte })
                .collect();
            let mut tokenizer = SqliTokenizer::new(&input, flags);
            let token = tokenizer.next_token().unwrap();
            assert_eq!(token.token_type, TokenType::String, "{input:?}");
            assert_eq!((token.pos, token.len, token.count), (0, content_len, 0), "{input:?}");
            let mut expected = [0; 32];
            expected[..content_len].copy_from_slice(&input[..content_len]);
            assert_eq!(token.val, expected, "{input:?}");
            assert_eq!(token.str_open, CHAR_NULL);
            assert_eq!(token.str_close, if closed { quote } else { CHAR_NULL }, "{input:?}");
            assert_eq!(tokenizer.pos, cursor, "{input:?}");
            if cursor < input.len() {
                let next = tokenizer.next_token().unwrap();
                assert_eq!((next.pos, next.len, next.val[0]), (cursor, 1, b'!'));
                assert_eq!(tokenizer.pos, input.len());
            }
            assert!(tokenizer.next_token().is_none());
        }
    }
}

#[test]
fn simulated_quote_search_keeps_full_cursor_and_truncated_token() {
    for (quote, flags) in [
        (b'\'', SqliFlags::FLAG_QUOTE_SINGLE),
        (b'"', SqliFlags::FLAG_QUOTE_DOUBLE),
    ] {
        let mut input = vec![b'a'; 64];
        input.push(quote);
        let mut tokenizer = SqliTokenizer::new(&input, flags);
        let token = tokenizer.next_token().unwrap();
        assert_eq!(token.token_type, TokenType::String);
        assert_eq!((token.pos, token.len, token.count), (0, 31, 0));
        assert_eq!(&token.val[..31], &[b'a'; 31]);
        assert_eq!(token.val[31], CHAR_NULL);
        assert_eq!((token.str_open, token.str_close), (CHAR_NULL, quote));
        assert_eq!(tokenizer.pos, 65);
        assert!(tokenizer.next_token().is_none());
    }
}

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
