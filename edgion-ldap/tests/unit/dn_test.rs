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
        "2.5.4.3=a",
        "cn=#0403616263",
        "cn=#0403616263 ,dc=x",
        "cn=#3003020101",
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
        // RFC 2253 `;` separator: rejected at admission.
        "cn=a;dc=x",
        // `#hex` that is not exactly one well-formed BER element.
        "cn=#616263",
        "cn=#616263 ,dc=x",
        "cn=#0405616263",
        "cn=#04",
        // Escapes that decode to invalid UTF-8.
        "cn=\\ff",
        // A one-arc OID is neither a descriptor nor a numeric OID.
        "5=a",
    ] {
        assert!(!is_valid(bad), "{bad}");
    }
}

#[test]
fn semicolon_separator_is_lenient_only_for_directory_dns() {
    assert!(normalize("cn=a;dc=x").is_some());
    assert!(equivalent("cn=a;dc=x", "cn=a,dc=x"));
    assert!(!is_string_form("cn=a;dc=x"));
}

#[test]
fn schema_aliases_compare_equal() {
    for (a, b) in [
        ("rfc822Mailbox=svc@corp,dc=x", "mail=svc@corp,dc=x"),
        ("0.9.2342.19200300.100.1.3=svc@corp,dc=x", "MAIL=svc@corp,dc=x"),
        ("emailAddress=svc@corp,dc=x", "email=svc@corp,dc=x"),
        ("pkcs9email=svc@corp,dc=x", "1.2.840.113549.1.9.1=svc@corp,dc=x"),
        ("mobileTelephoneNumber=1,dc=x", "mobile=1,dc=x"),
        ("fax=1,dc=x", "facsimileTelephoneNumber=1,dc=x"),
        ("2.5.4.23=1,dc=x", "fax=1,dc=x"),
        ("friendlyCountryName=x,dc=x", "co=x,dc=x"),
        ("aliasedEntryName=x,dc=x", "aliasedObjectName=x,dc=x"),
        ("2.5.4.5=7,dc=x", "serialNumber=7,dc=x"),
        ("2.5.4.41=x,dc=x", "name=x,dc=x"),
        ("2.5.4.20=1,dc=x", "telephoneNumber=1,dc=x"),
        ("0.9.2342.19200300.100.1.44=x,dc=x", "uniqueIdentifier=x,dc=x"),
        ("2.5.4.17=1,dc=x", "postalCode=1,dc=x"),
        ("0.9.2342.19200300.100.1.41=1,dc=x", "mobile=1,dc=x"),
        ("0.9.2342.19200300.100.1.43=x,dc=x", "co=x,dc=x"),
        ("2.5.4.1=x,dc=x", "aliasedObjectName=x,dc=x"),
        ("2.5.4.43=x,dc=x", "initials=x,dc=x"),
        ("2.5.4.44=x,dc=x", "generationQualifier=x,dc=x"),
        ("2.5.4.46=x,dc=x", "dnQualifier=x,dc=x"),
        ("2.5.4.65=x,dc=x", "pseudonym=x,dc=x"),
        ("2.5.4.15=x,dc=x", "businessCategory=x,dc=x"),
    ] {
        assert!(equivalent(a, b), "{a} vs {b}");
    }
    // Distinct attributes stay distinct.
    assert!(!equivalent("mail=svc@corp,dc=x", "email=svc@corp,dc=x"));
    assert!(!equivalent("cn=x,dc=x", "name=x,dc=x"));
}

#[test]
fn hex_string_values() {
    // Two-octet BER length (0x82) for a 300-byte value.
    let long = "a".repeat(300);
    let hex: String = long.bytes().map(|b| format!("{b:02x}")).collect();
    assert!(equivalent(
        &format!("cn=#0482012c{hex},dc=x"),
        &format!("cn={long},dc=x")
    ));
    // BMPString (UTF-16BE) and UniversalString (UTF-32BE).
    assert!(equivalent("cn=#1e06005300760043,dc=x", "cn=svc,dc=x"));
    assert!(equivalent("cn=#1c0c000000730000007600000063,dc=x", "cn=SVC,dc=x"));
    // A well-formed non-string element, or string content that is not text,
    // is opaque: equal only to the same bytes, never to a text value.
    assert!(equivalent("cn=#3003020101,dc=x", "cn=#3003020101,dc=x"));
    // Opaque raw bytes `a`, SPACE, 32 x `a` (tag 0x61, length 0x20) equal the
    // folded bytes of the text value "a aaa...": only the kind keeps them apart.
    let thirty_two = "a".repeat(32);
    let hex: String = thirty_two.bytes().map(|b| format!("{b:02x}")).collect();
    assert!(normalize(&format!("cn=#6120{hex}")).is_some());
    assert!(!equivalent(&format!("cn=#6120{hex}"), &format!("cn=a {thirty_two}")));
    // Invalid text in a string type stays opaque (never decoded lossily to U+FFFD).
    for opaque in [
        "cn=#0401ff",
        "cn=#1e02d800",
        "cn=#1c040000d800",
        "cn=#1c0400110000",
        "cn=#1e03007300",
    ] {
        assert!(normalize(opaque).is_some(), "{opaque}");
        assert!(!equivalent(opaque, "cn=\u{FFFD}"), "{opaque}");
    }
    assert!(!equivalent("cn=#1e03007300", "cn=s"));
}

#[test]
fn hex_values_must_be_exactly_one_element() {
    for bad in [
        // Trailing bytes after the element.
        "cn=#04016162",
        // A two-octet length that disagrees with the content.
        "cn=#048200056162",
        // Indefinite and three-octet length forms.
        "cn=#0480",
        "cn=#0483000003616263",
        // High-tag-number identifiers, well-formed or not.
        "cn=#1f200141",
        "cn=#1f0141",
    ] {
        assert!(normalize(bad).is_none(), "{bad}");
        assert!(!is_valid(bad), "{bad}");
    }
}

#[test]
fn rfc4518_mapping_and_case() {
    let service = "cn=svc acct,dc=x";
    for same in [
        // Mapped to space: NO-BREAK SPACE, CHARACTER TABULATION.
        "cn=svc\u{00A0}acct,dc=x",
        "cn=svc\tacct,dc=x",
        // Mapped to nothing: SOFT HYPHEN, ZERO WIDTH SPACE, DELETE, BOM.
        "cn=svc a\u{00AD}cct,dc=x",
        "cn=svc\u{200B} acct,dc=x",
        "cn=svc acct\u{007F},dc=x",
        "cn=\u{FEFF}svc acct,dc=x",
    ] {
        assert!(equivalent(service, same), "{same:?}");
    }
    // Context-free lowercasing: a word-final capital sigma and a stored
    // non-final small sigma agree (context-sensitive `str::to_lowercase` would
    // turn the capital into final sigma), and final sigma folds to sigma.
    assert!(equivalent("cn=\u{03BF}\u{03C3},dc=x", "cn=\u{039F}\u{03A3},dc=x"));
    assert!(equivalent("cn=\u{03BF}\u{03C2},dc=x", "cn=\u{039F}\u{03A3},dc=x"));
}

#[test]
fn normalized_dn_debug_is_redacted() {
    let rendered = format!("{:?}", normalize("uid=secret-user,dc=x").unwrap());
    assert!(!rendered.contains("secret"), "{rendered}");
    assert!(rendered.contains("[redacted]"), "{rendered}");
}
