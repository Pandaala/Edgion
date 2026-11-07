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

/// waf4-01: the 3-token keyword whitelist must compare the `INTO` prefix
/// ASCII-case-insensitively, like libinjection C `cstrcasecmp` and
/// libinjection-go `toUpperCmp`. Expected verdicts were produced by the
/// libinjection-go v0.3.2 and v0.3.3 oracles on 2026-11-07.
#[test]
fn three_token_into_outfile_whitelist_is_case_insensitive() {
    for (input, fingerprint) in [
        (b"x' INTO OUTFILE '/var/www/s.php".as_slice(), "sks"),
        (b"x' into outfile '/var/www/s.php".as_slice(), "sks"),
        (b"x' Into Dumpfile '/tmp/a".as_slice(), "sks"),
        (b"1 into outfile 'x'".as_slice(), "1ks"),
        (b"'a' into dumpfile user0x1f".as_slice(), "skn"),
    ] {
        let mut state = SqliState::new(input, SqliFlags::FLAG_NONE);
        assert!(state.detect(), "{}", input.escape_ascii());
        assert_eq!(state.fingerprint_string(), fingerprint, "{}", input.escape_ascii());
    }
}

/// F-S4-5: `merge_words` skips a merge whose joined length is `>= 32`
/// (libinjection C bound) while libinjection-go uses `> 32`. The difference is
/// unobservable only while every keyword table entry is shorter than 32 bytes.
#[test]
fn keyword_table_entries_fit_the_merge_buffer() {
    let longest = sqli_data::SQL_KEYWORDS
        .iter()
        .map(|entry| entry.word.len())
        .max()
        .unwrap_or(0);
    assert!(longest < 32, "longest keyword is {longest} bytes");
}
