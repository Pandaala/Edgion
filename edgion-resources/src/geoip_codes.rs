//! Static ISO 3166-1 alpha-2 and continent code tables.
//!
//! Lives in `edgion-resources` so the controller can validate country codes at
//! resource ingest without linking an MMDB reader. Matching is exact and
//! uppercase; callers normalize before querying.

/// The seven continent codes MaxMind emits in `continent.code`.
pub const CONTINENT_CODES: &[&str] = &["AF", "AN", "AS", "EU", "NA", "OC", "SA"];

/// ISO 3166-1 alpha-2 plus the user-assigned codes MaxMind actually emits in
/// `country.iso_code`, sorted so the lookup can binary-search. Do not "correct"
/// the user-assigned additions back out — see each one's comment for why it's
/// here.
///
/// Checked and deliberately NOT added: MaxMind's legacy GeoIP CSV pseudo-country
/// codes `A1` (anonymous proxy) / `A2` (satellite provider) do not appear in
/// `country.iso_code` in the MMDB/GeoIP2 format this project reads — MaxMind
/// migrated those to boolean `traits` fields (`is_anonymous_proxy`,
/// `is_satellite_provider`) years ago, and the `maxminddb` crate's
/// `geoip2::City` record this codebase decodes has no path back to the legacy
/// values. Likewise `EU` and `AP`: `EU` is a real value, but only in
/// `continent.code` (already covered by `CONTINENT_CODES` above), never in
/// `country.iso_code`; `AP` ("Asia/Pacific Region") was a GeoIP Legacy-only
/// pseudo-country with no equivalent in the modern MMDB format at all.
const COUNTRY_CODES: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ", "BA", "BB", "BD",
    "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS", "BT", "BV", "BW", "BY", "BZ", "CA",
    "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN", "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE",
    "DJ", "DK", "DM", "DO", "DZ", "EC", "EE", "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA",
    "GB", "GD", "GE", "GF", "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK",
    "HM", "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM", "JO", "JP",
    "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC", "LI", "LK", "LR", "LS", "LT",
    "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK", "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS",
    "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA", "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ",
    "OM", "PA", "PE", "PF", "PG", "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS",
    "RU", "RW", "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS", "ST",
    "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO", "TR", "TT", "TV", "TW",
    "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI", "VN", "VU", "WF", "WS",
    // User-assigned (not ISO 3166-1): Kosovo has no official ISO 3166-1 code,
    // and MaxMind (along with the EU and many other organizations) uses this
    // de facto value in `country.iso_code` for it.
    "XK", "YE", "YT", "ZA", "ZM", "ZW",
];

/// Exact, case-sensitive membership test. Uppercase before calling.
pub fn is_valid_country(code: &str) -> bool {
    COUNTRY_CODES.binary_search(&code).is_ok()
}

/// Exact, case-sensitive membership test. Uppercase before calling.
pub fn is_valid_continent(code: &str) -> bool {
    CONTINENT_CODES.contains(&code)
}

#[cfg(test)]
mod tests {
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
}
