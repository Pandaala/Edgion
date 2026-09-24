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
#[path = "../tests/unit/geoip_codes_test.rs"]
mod tests;
