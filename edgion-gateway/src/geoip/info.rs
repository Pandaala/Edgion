//! Resolved geographic attributes and the MMDB record projection.
//!
//! Value provenance is normative — an unpinned choice silently breaks matching
//! across database versions and locales:
//!   country     <- country.iso_code           (ISO 3166-1 alpha-2)
//!   continent   <- continent.code
//!   subdivision <- subdivisions[0].iso_code   (first level only)
//!   city        <- city.names["en"]
//!
//! There is deliberately NO fallback to `registered_country` or
//! `represented_country`: those describe the registrant, not the client's
//! location, and conflating them would make one country rule mean two different
//! things for different clients.

use std::net::IpAddr;
use std::sync::Arc;

/// Resolved attributes, modelled as the superset of a City-tier database.
/// A Country-tier database yields `None` for subdivision, city, and coordinates.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GeoInfo {
    pub country_iso: Option<String>,
    pub continent: Option<String>,
    pub subdivision: Option<String>,
    pub city: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// The four matchable dimensions, parsed from a rule's `key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoDimension {
    Country,
    Continent,
    City,
    Subdivision,
}

impl GeoDimension {
    /// Parse a config `key`. Returns `None` for anything else; the caller
    /// rejects that at ingest rather than guessing.
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "country" => Some(Self::Country),
            "continent" => Some(Self::Continent),
            "city" => Some(Self::City),
            "subdivision" => Some(Self::Subdivision),
            _ => None,
        }
    }
}

impl GeoInfo {
    pub fn dimension(&self, dim: GeoDimension) -> Option<&str> {
        match dim {
            GeoDimension::Country => self.country_iso.as_deref(),
            GeoDimension::Continent => self.continent.as_deref(),
            GeoDimension::City => self.city.as_deref(),
            GeoDimension::Subdivision => self.subdivision.as_deref(),
        }
    }
}

/// The outcome of one lookup.
///
/// `Unavailable` and `NotFound` drive different policies everywhere downstream:
/// "the security control is not running" must stay distinguishable from "this
/// client is in a private range".
#[derive(Debug, Clone, PartialEq)]
pub enum GeoLookup {
    /// No database is loaded at all.
    Unavailable,
    /// A database is loaded but has no record for this address.
    NotFound,
    Found(Arc<GeoInfo>),
}

/// Collapse an IPv4-mapped IPv6 literal to its IPv4 form.
///
/// On a dual-stack listener the socket peer is frequently `::ffff:a.b.c.d`, and
/// many MMDB builds have no record for the mapped form — without this every
/// client would silently resolve to "unknown".
pub fn normalize_addr(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Project an MMDB City record onto `GeoInfo` using the pinned field choices.
///
/// The field shapes below are those of `maxminddb` 0.30, verified against the
/// working fixture readback test: `country` / `continent` / `city` / `location`
/// are plain (non-`Option`) sub-structs whose leaves are `Option`, `subdivisions`
/// is a `Vec`, and localized names are a typed struct with an `english` field
/// rather than a `BTreeMap` keyed by language code.
///
/// Empty strings are filtered out: an MMDB record can carry a present-but-empty
/// field, and treating that as a value would make a rule match on `""`.
pub fn project(rec: &maxminddb::geoip2::City) -> GeoInfo {
    GeoInfo {
        country_iso: rec
            .country
            .iso_code
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase()),
        continent: rec
            .continent
            .code
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase()),
        subdivision: rec
            .subdivisions
            .first()
            .and_then(|s| s.iso_code)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase()),
        city: rec.city.names.english.filter(|s| !s.is_empty()).map(|s| s.to_string()),
        latitude: rec.location.latitude,
        longitude: rec.location.longitude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn ipv4_mapped_ipv6_normalizes_to_ipv4() {
        let mapped = IpAddr::V6("::ffff:203.0.113.5".parse::<Ipv6Addr>().unwrap());
        let plain = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5));
        assert_eq!(normalize_addr(mapped), plain);
        assert_eq!(normalize_addr(plain), plain);
    }

    #[test]
    fn genuine_ipv6_is_left_alone() {
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(normalize_addr(v6), v6);
    }

    #[test]
    fn dimension_reads_the_pinned_fields() {
        let info = GeoInfo {
            country_iso: Some("CN".into()),
            continent: Some("AS".into()),
            subdivision: Some("BJ".into()),
            city: Some("Beijing".into()),
            latitude: Some(39.9),
            longitude: Some(116.4),
        };
        assert_eq!(info.dimension(GeoDimension::Country), Some("CN"));
        assert_eq!(info.dimension(GeoDimension::Continent), Some("AS"));
        assert_eq!(info.dimension(GeoDimension::Subdivision), Some("BJ"));
        assert_eq!(info.dimension(GeoDimension::City), Some("Beijing"));
    }

    #[test]
    fn absent_dimensions_report_none() {
        let info = GeoInfo::default();
        assert_eq!(info.dimension(GeoDimension::City), None);
        assert_eq!(info.dimension(GeoDimension::Country), None);
    }

    #[test]
    fn dimension_parses_from_config_key() {
        assert_eq!(GeoDimension::parse("country"), Some(GeoDimension::Country));
        assert_eq!(GeoDimension::parse("continent"), Some(GeoDimension::Continent));
        assert_eq!(GeoDimension::parse("city"), Some(GeoDimension::City));
        assert_eq!(GeoDimension::parse("subdivision"), Some(GeoDimension::Subdivision));
        assert_eq!(GeoDimension::parse("region"), None);
    }
}
