use super::*;

#[test]
fn known_countries_are_valid() {
    assert!(is_valid_country("CN"));
    assert!(is_valid_country("US"));
    assert!(is_valid_country("SE"));
    assert!(is_valid_country("GB"));
}

#[test]
fn unknown_and_malformed_countries_are_invalid() {
    assert!(!is_valid_country("ZZ"));
    assert!(!is_valid_country("CHN"));
    assert!(!is_valid_country(""));
    // The caller uppercases first; the table itself is exact-match.
    assert!(!is_valid_country("cn"));
}

/// `XK` is a user-assigned (not ISO 3166-1) code MaxMind emits in
/// `country.iso_code` for Kosovo. Before this addition, a `denyCountries:
/// ["XK"]` rule (a legitimate, working config) triggered a spurious "not a
/// known country code" WARN — this is the regression guard.
#[test]
fn kosovo_user_assigned_code_is_valid() {
    assert!(is_valid_country("XK"));
}

/// `EU`/`AP` were considered and deliberately excluded (see the table's
/// doc comment): `EU` is a `continent.code` value, never emitted in
/// `country.iso_code`; `AP` is a GeoIP Legacy-only pseudo-country with no
/// equivalent in the MMDB format this project reads. Neither belongs in
/// this table, and this test documents that decision as a guard against
/// someone adding them without re-deriving the reasoning.
#[test]
fn eu_and_ap_are_not_country_codes() {
    assert!(!is_valid_country("AP"));
    // "EU" is intentionally absent from COUNTRY_CODES even though it IS a
    // valid CONTINENT_CODES entry — the two tables serve different MMDB
    // fields and must not be conflated.
    assert!(!is_valid_country("EU"));
    assert!(is_valid_continent("EU"));
}

#[test]
fn continents_are_the_seven_codes() {
    for c in ["AF", "AN", "AS", "EU", "NA", "OC", "SA"] {
        assert!(is_valid_continent(c), "{c} should be valid");
    }
    assert!(!is_valid_continent("XX"));
}

#[test]
fn country_table_is_sorted_for_binary_search() {
    let mut sorted = COUNTRY_CODES.to_vec();
    sorted.sort_unstable();
    assert_eq!(COUNTRY_CODES, sorted.as_slice(), "COUNTRY_CODES must stay sorted");
}
