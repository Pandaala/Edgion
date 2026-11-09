use super::*;

#[test]
fn escapes_match_ldap3_hex_form() {
    assert_eq!(escape("alice,admin"), r"alice\2cadmin");
    assert_eq!(escape("alice+admin"), r"alice\2badmin");
    assert_eq!(escape("#bob "), r"\23bob\20");
    assert_eq!(escape(" "), r"\20");
    assert_eq!(escape("# "), r"\23\20");
    assert_eq!(escape("a\"+,;<=>\\\0b"), r"a\22\2b\2c\3b\3c\3d\3e\5c\00b");
}

#[test]
fn unescaped_values_are_borrowed() {
    for value in ["", "alice", "a#b", "a b", "Zoë Ünïcödé", "José Müller"] {
        assert!(matches!(escape(value), Cow::Borrowed(v) if v == value), "{value}");
    }
}

#[test]
fn non_ascii_is_copied_around_escapes() {
    assert_eq!(escape("é,ñ "), r"é\2cñ\20");
}
