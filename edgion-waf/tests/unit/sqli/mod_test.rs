use super::*;

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
