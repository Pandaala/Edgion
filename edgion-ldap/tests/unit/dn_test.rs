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

#[test]
fn dn_equivalent_forms() {
    let canonical = "cn=svc,ou=Svc,dc=corp,dc=com";
    for same in [
        "CN=svc,OU=svc,DC=corp,DC=com",
        "cn=svc, ou=Svc , dc=corp,dc=com",
        " cn = svc ,ou=svc,dc=CORP,dc=com ",
        "cn=sv\\63,ou=svc,dc=corp,dc=com",
        "cn=svc,ou=svc,dc=corp,dc=com",
    ] {
        assert!(equivalent(canonical, same), "{same}");
    }
    for different in [
        "cn=svc2,ou=svc,dc=corp,dc=com",
        "cn=svc,dc=corp,dc=com",
        "uid=svc,ou=svc,dc=corp,dc=com",
        "svc@corp.com",
        "",
    ] {
        assert!(!equivalent(canonical, different), "{different}");
    }
    // Multi-valued RDNs compare independently of AVA order.
    assert!(equivalent("cn=a+uid=b,dc=x", "uid=b+cn=a,dc=x"));
    // Escaped separators are part of the value.
    assert!(equivalent("cn=a\\,b,dc=x", "cn=a\\2cb,dc=x"));
    // The last RDN is sorted too; `;` separates RDNs like `,`.
    assert!(equivalent("dc=x,cn=a+uid=b", "dc=x;uid=b+cn=a"));
}

#[test]
fn dn_equivalence_covers_directory_spellings() {
    let service = "cn=svc acct,ou=svc,dc=corp,dc=com";
    for same in [
        // Type aliases and OIDs of the common naming attributes.
        "2.5.4.3=svc acct,2.5.4.11=svc,0.9.2342.19200300.100.1.25=corp,dc=com",
        "commonName=svc acct,organizationalUnitName=svc,domainComponent=corp,DC=com",
        // RFC 4518 insignificant spaces, including escaped ones.
        "cn=svc   acct,ou=svc,dc=corp,dc=com",
        "cn=\\20svc acct\\20,ou=svc,dc=corp,dc=com",
        // An escape right after a space keeps the space significant.
        "cn=svc \\61cct,ou=svc,dc=corp,dc=com",
        // BER `#hex` strings (OCTET STRING, UTF8String).
        "cn=#0409737663206163637420,ou=svc,dc=corp,dc=com",
        "cn=#0C09535643204143435420,ou=svc,dc=corp,dc=com",
    ] {
        assert!(equivalent(service, same), "{same}");
    }
    assert!(equivalent("cn=J\u{fc}rgen,dc=x", "CN=J\u{dc}RGEN,dc=x"));
    assert!(equivalent("uid=a,dc=x", "userid=a,dc=x"));
    // Spaces between `=` and a `#hex` value are skipped before the `#` check.
    assert!(equivalent("cn= #0403616263,dc=x", "cn=abc,dc=x"));
    // Long-form BER length (0x81) for a 200-byte value.
    let long = "a".repeat(200);
    let hex: String = long.bytes().map(|b| format!("{b:02x}")).collect();
    assert!(equivalent(&format!("cn=#0481c8{hex},dc=x"), &format!("cn={long},dc=x")));
    assert!(!equivalent("cn=svcacct,dc=x", "cn=svc acct,dc=x"));
}

#[test]
fn string_form() {
    assert!(is_string_form("cn=svc,ou=x,dc=corp"));
    assert!(is_string_form("commonName=svc"));
    assert!(!is_string_form("2.5.4.3=svc"));
    assert!(!is_string_form("cn=#0403737663"));
    assert!(!is_string_form("svc@corp.com"));
}

#[test]
fn dn_validity() {
    for good in [
        "cn=a",
        "cn=a,dc=x",
        "cn=a;dc=x",
        "2.5.4.3=a",
        "cn=#616263",
        "cn=#616263 ,dc=x",
        "cn=a+sn=b",
        "cn=,dc=x",
        "cn=a=b",
    ] {
        assert!(is_valid(good), "{good}");
    }
    for bad in [
        "",
        "svc@corp.com",
        "CORP\\svc",
        "cn=a,",
        "cn=#41 xou=b",
        "cn=#",
        "cn=a\"b",
        "cn=<a>",
        "cn=a\0b",
        "=a",
        "c n=a",
        "cn=a\\",
        "cn=a\\x",
        "cn=#6",
    ] {
        assert!(!is_valid(bad), "{bad}");
    }
}
