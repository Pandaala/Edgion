use super::*;

#[test]
fn whitelist_dispatch_preserves_fingerprint_view_and_statistics() {
    for (fingerprint, token_count, expected) in [
        (b"1U".as_slice(), 2, false),
        (b"1U".as_slice(), 3, true),
        (b"1U\0".as_slice(), 2, false),
        (b"1&1".as_slice(), 3, false),
        (b"1&1".as_slice(), 4, true),
        (b"sos".as_slice(), 3, false),
        (b"s&s".as_slice(), 3, false),
        (b"\xffU".as_slice(), 2, true),
        (b"1\0U".as_slice(), 2, true),
        (b"".as_slice(), 0, true),
    ] {
        let mut state = SqliState::new(b"", SqliFlags::FLAG_NONE);
        state.fingerprint[..fingerprint.len()].copy_from_slice(fingerprint);
        state.tokens.resize(3, Token::new());
        state.tokens[0].str_open = b'\'';
        state.tokens[0].str_close = b'\'';
        state.tokens[2].str_open = b'\'';
        state.tokens[2].str_close = b'\'';
        state.stats_tokens = token_count;
        assert_eq!(
            state.is_not_whitelist(),
            expected,
            "{fingerprint:?}, count {token_count}"
        );
    }
}

#[test]
fn password_marker_search_is_case_sensitive_across_raw_input() {
    for (input, expected) in [
        (b"".as_slice(), false),
        (b"sp_passwor".as_slice(), false),
        (b"sp_password".as_slice(), true),
        (b"sp_password tail".as_slice(), true),
        (b"prefix sp_password".as_slice(), true),
        (b"sp_passworsp_passwordsp_password".as_slice(), true),
        (b"SP_PASSWORD".as_slice(), false),
        (b"sp_Password".as_slice(), false),
        (b"\0\xffsp_password\0".as_slice(), true),
        (b"sp_pass\0word".as_slice(), false),
        (b"sp_pass\xffword".as_slice(), false),
        (b"prefix sp_passwor tail".as_slice(), false),
    ] {
        let state = SqliState::new(input, SqliFlags::FLAG_NONE);
        assert_eq!(state.contains_sp_password(), expected, "{input:?}");
    }

    for length in [63, 64, 65, 256] {
        let mut input = vec![b'x'; length];
        assert!(!SqliState::new(&input, SqliFlags::FLAG_NONE).contains_sp_password());
        input[length - 11..].copy_from_slice(b"sp_password");
        assert!(SqliState::new(&input, SqliFlags::FLAG_NONE).contains_sp_password());
    }
}
