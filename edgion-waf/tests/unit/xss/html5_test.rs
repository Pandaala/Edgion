use super::*;

#[test]
fn percent_comments_preserve_borrowed_tokens_and_first_adjacent_ending() {
    for (input, comment, cursor, tail) in [
        (b"<%".as_slice(), b"".as_slice(), 2, b"".as_slice()),
        (b"<%%>".as_slice(), b"".as_slice(), 4, b"".as_slice()),
        (b"<%%>tail".as_slice(), b"".as_slice(), 4, b"tail".as_slice()),
        (b"<%a%x%>tail".as_slice(), b"a%x".as_slice(), 7, b"tail".as_slice()),
        (b"<%a%%>tail".as_slice(), b"a%".as_slice(), 6, b"tail".as_slice()),
        (b"<%a%".as_slice(), b"a%".as_slice(), 4, b"".as_slice()),
        (b"<%a>".as_slice(), b"a>".as_slice(), 4, b"".as_slice()),
        (b"<%a%\0>".as_slice(), b"a%\0>".as_slice(), 6, b"".as_slice()),
        (
            b"<%\0\xff%>tail".as_slice(),
            b"\0\xff".as_slice(),
            6,
            b"tail".as_slice(),
        ),
        (b"<%a%>%>".as_slice(), b"a".as_slice(), 5, b"%>".as_slice()),
    ] {
        let mut html = Html5State::new(input, Html5Flags::DataState);
        assert!(html.next(), "{input:?}");
        assert_eq!(html.token_type, TokenType::TagComment);
        assert_eq!(html.token_start.as_ptr(), input[2..].as_ptr());
        assert_eq!(html.token_start, &input[2..]);
        assert_eq!(html.token_len, comment.len());
        assert_eq!(&html.token_start[..html.token_len], comment);
        assert_eq!(html.position(), cursor);
        if !tail.is_empty() {
            assert!(html.next());
            assert_eq!(html.token_type, TokenType::DataText);
            assert_eq!(html.token_len, tail.len());
            assert_eq!(&html.token_start[..html.token_len], tail);
            assert_eq!(html.position(), input.len());
        }
        assert!(!html.next());
    }

    let mut input = b"<%".to_vec();
    input.extend_from_slice(&[b'%'; 128]);
    input.extend_from_slice(b">tail");
    let mut html = Html5State::new(&input, Html5Flags::DataState);
    assert!(html.next());
    assert_eq!(html.token_type, TokenType::TagComment);
    assert_eq!(html.token_start.as_ptr(), input[2..].as_ptr());
    assert_eq!(html.token_len, 127);
    assert_eq!(&html.token_start[..html.token_len], &[b'%'; 127]);
    assert_eq!(html.position(), 131);
    assert!(html.next());
    assert_eq!(html.token_type, TokenType::DataText);
    assert_eq!(&html.token_start[..html.token_len], b"tail");
    assert_eq!(html.position(), input.len());
    assert!(!html.next());
}

#[test]
fn byte_search_preserves_first_match_and_absolute_offsets() {
    let input = b"\0\xff<<'\"`%>>";
    let html = Html5State::new(input, Html5Flags::DataState);
    for (byte, start, expected) in [
        (0, 0, Some(0)),
        (0, 1, None),
        (0xff, 0, Some(1)),
        (b'<', 0, Some(2)),
        (b'<', 3, Some(3)),
        (b'<', 4, None),
        (b'\'', 0, Some(4)),
        (b'"', 0, Some(5)),
        (b'`', 0, Some(6)),
        (b'%', 0, Some(7)),
        (b'>', 0, Some(8)),
        (b'>', 9, Some(9)),
        (b'>', input.len(), None),
        (b'>', input.len() + 1, None),
        (b'>', usize::MAX, None),
        (b'x', 0, None),
    ] {
        assert_eq!(html.find_byte(byte, start), expected, "byte {byte}, start {start}");
    }
    assert_eq!(Html5State::new(b"", Html5Flags::DataState).find_byte(0, 0), None);
}

#[test]
fn cdata_search_preserves_overlapping_prefixes_and_short_tails() {
    for (input, start, expected) in [
        (b"".as_slice(), 0, None),
        (b"]".as_slice(), 0, None),
        (b"]]".as_slice(), 0, None),
        (b"]>]".as_slice(), 0, None),
        (b"]]>".as_slice(), 0, Some(0)),
        (b"]]>".as_slice(), 1, None),
        (b"]]>]]>".as_slice(), 0, Some(0)),
        (b"]]>]]>".as_slice(), 1, Some(3)),
        (b"x]]]>a]]>".as_slice(), 0, Some(2)),
        (b"x]]]>a]]>".as_slice(), 2, Some(2)),
        (b"x]]]>a]]>".as_slice(), 3, Some(6)),
        (b"x]]]>a]]>".as_slice(), 6, Some(6)),
        (b"x]]]>a]]>".as_slice(), 7, None),
        (b"x]]]>a]]>".as_slice(), 9, None),
        (b"x]]]>a]]>".as_slice(), 10, None),
        (b"\xff\0]]>".as_slice(), 0, Some(2)),
        // F-S4-3: lone right brackets after a CDATA opener (libinjection-go
        // v0.3.2 panics here; C and v0.3.3 return no XSS verdict).
        (b"<![CDATA[]]]".as_slice(), 9, None),
        (b"<![CDATA[]x]]".as_slice(), 9, None),
    ] {
        let html = Html5State::new(input, Html5Flags::DataState);
        assert_eq!(html.find_cdata_end(start), expected, "{input:?}, start {start}");
    }
}

#[test]
fn high_byte_is_data_in_all_whitespace_skip_states() {
    for input in [
        b"<a \xffid=x><script>".as_slice(),
        b"<a id \xff=x><script>".as_slice(),
        b"<a id= \xff><script>".as_slice(),
    ] {
        let mut html = Html5State::new(input, Html5Flags::DataState);
        let mut found_script = false;
        let mut tokens = 0;
        while html.next() {
            tokens += 1;
            assert!(tokens <= input.len() * 2);
            if html.token_type == TokenType::TagNameOpen && html.token_start.starts_with(b"script") {
                found_script = true;
            }
        }
        assert!(found_script, "tokenizer stopped before script for {input:?}");
    }
}

#[test]
fn eof_in_all_whitespace_skip_states_terminates() {
    for input in [b"<a ".as_slice(), b"<a id ".as_slice(), b"<a id= ".as_slice()] {
        let mut html = Html5State::new(input, Html5Flags::DataState);
        let mut tokens = 0;
        while html.next() {
            tokens += 1;
            assert!(tokens <= input.len() * 2);
        }
        assert_eq!(html.position(), input.len());
    }
}

impl<'a> Html5State<'a> {
    #[cfg(test)]
    pub fn debug_is_close(&self) -> bool {
        self.is_close
    }
    #[cfg(test)]
    pub fn debug_pos(&self) -> usize {
        self.pos
    }
}
