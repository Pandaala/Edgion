#!/usr/bin/env python3
"""Generate the synthetic MMDB test fixtures used by GeoIP country-restriction tests.

Run once, by hand, with the pinned toolchain from README.md:

    python3 -m venv /tmp/mmdbgen
    /tmp/mmdbgen/bin/pip install mmdb_writer==0.2.7 netaddr maxminddb
    /tmp/mmdbgen/bin/python3 generate.py

This script is not part of the CI/build path (same arrangement as
edgion-tests/wasm/build.sh, which needs a toolchain nobody installs in CI).
It never resolves real MaxMind data: real MaxMind test databases only cover
public IP ranges, but integration test clients always connect from loopback
and RFC1918 addresses. This fixture maps exactly those ranges to synthetic
countries so the whole GeoIP decision table is reachable from a closed
integration-test harness, and being self-authored, needs no third-party
redistribution review.

The address map below is normative for the whole GeoIP country-restriction
feature plan; every probe address used by any later task's tests comes from
this table. Do not edit it without updating every consumer.

| Network                | country | continent | city     | subdivision | coords |
|-------------------------|---------|-----------|----------|-------------|--------|
| 127.0.0.1/32            | CN      | AS        | Beijing  | BJ          | yes    |
| 127.0.0.5/32            | RU      | EU        | Moscow   | MOW         | yes    |
| 127.0.0.6/32            | US      | NA        | (none)   | (none)      | no     |
| 127.0.0.7/32            | HK      | AS        | Kowloon  | KOW         | yes    |
| 10.0.0.0/8              | US      | NA        | Cambridge| MA          | yes    |
| 203.0.113.0/24          | GB      | EU        | London   | ENG         | yes    |
| 198.51.100.0/24         | HK      | AS        | (none)   | (none)      | no     |
| anything else (incl. 8.8.8.8, 127.0.0.2-4) | unmapped | | | | |

127.0.0.6 and 198.51.100.0/24 are country-only on purpose: even though other
records in the fixture carry city data, these records lack city, subdivision,
and location fields and therefore exercise the ordinary per-record onMissing
path. 8.8.8.8 is the GeoLookup::NotFound probe.

Deliberately absent: no record at ::ffff:0:0/96. The absence reproduces the
exact IPv4-mapped-address pathology the design calls out, which makes
normalize_addr's ::ffff:0:0/96 handling a falsifiable assertion instead of a
vacuous one (this database uses ipv4_compatible=True, i.e. IPv4 networks are
stored under the ::/96 "compatible" prefix, never under the ::ffff:0:0/96
"mapped" prefix -- so a reader that forgets to normalize an incoming mapped
address will not find these records).
"""

import time

import mmdb_writer

# Pin build_epoch so re-running this script is byte-reproducible. The library
# hardcodes `int(time.time())` inside MMDBWriter._build_meta with no override
# hook, so we monkeypatch time.time for the duration of this one-shot script.
FIXED_BUILD_EPOCH = 1755043200  # 2026-08-13T00:00:00Z, arbitrary fixed point
time.time = lambda: FIXED_BUILD_EPOCH

# Distinct `database_type` identities let readback tests detect an accidentally
# swapped fixture. The gateway does not infer record capabilities from this
# free-text metadata. Keep these values in lockstep with README.md.
CITY_DATABASE_TYPE = "Edgion-Test-City"
COUNTRY_DATABASE_TYPE = "Edgion-Test-Country"


def city_record(country, continent, city_name, subdivision_iso, lat, lon):
    """Build a full City-tier record: country + continent + city + subdivision + location."""
    return {
        "country": {"iso_code": country, "names": {"en": country}},
        "continent": {"code": continent, "names": {"en": continent}},
        "city": {"names": {"en": city_name}},
        "subdivisions": [{"iso_code": subdivision_iso, "names": {"en": subdivision_iso}}],
        "location": {"latitude": lat, "longitude": lon, "accuracy_radius": 100},
    }


def country_only_record(country, continent):
    """Build a City-tier record with only country + continent (no city/subdivision/location).

    Used for the two probes that deliberately exercise the per-record
    dimension-absence path on a City-tier database.
    """
    return {
        "country": {"iso_code": country, "names": {"en": country}},
        "continent": {"code": continent, "names": {"en": continent}},
    }


def country_tier_record(country, continent):
    """Build a Country-tier record: country + continent only (no city-level schema at all)."""
    return {
        "country": {"iso_code": country, "names": {"en": country}},
        "continent": {"code": continent, "names": {"en": continent}},
    }


# The address map (network -> (country, continent, city, subdivision, lat, lon)).
# `None` city/subdivision/coords means "country-only" per the table above.
ADDRESS_MAP = [
    ("127.0.0.1/32", "CN", "AS", "Beijing", "BJ", 39.9042, 116.4074),
    ("127.0.0.5/32", "RU", "EU", "Moscow", "MOW", 55.7558, 37.6173),
    ("127.0.0.6/32", "US", "NA", None, None, None, None),
    ("127.0.0.7/32", "HK", "AS", "Kowloon", "KOW", 22.3193, 114.1694),
    ("10.0.0.0/8", "US", "NA", "Cambridge", "MA", 42.3736, -71.1097),
    ("203.0.113.0/24", "GB", "EU", "London", "ENG", 51.5074, -0.1278),
    ("198.51.100.0/24", "HK", "AS", None, None, None, None),
]


def build_city_db():
    writer = mmdb_writer.MMDBWriter(
        ip_version=6,
        ipv4_compatible=True,
        languages=["en"],
        database_type=CITY_DATABASE_TYPE,
        description="Edgion synthetic City-tier GeoIP test fixture",
    )
    for network, country, continent, city, subdivision, lat, lon in ADDRESS_MAP:
        if city is None:
            content = country_only_record(country, continent)
        else:
            content = city_record(country, continent, city, subdivision, lat, lon)
        writer.insert_network(mmdb_writer.IPSet([network]), content)
    return writer


def build_country_db():
    writer = mmdb_writer.MMDBWriter(
        ip_version=6,
        ipv4_compatible=True,
        languages=["en"],
        database_type=COUNTRY_DATABASE_TYPE,
        description="Edgion synthetic Country-tier GeoIP test fixture",
    )
    for network, country, continent, _city, _subdivision, _lat, _lon in ADDRESS_MAP:
        writer.insert_network(mmdb_writer.IPSet([network]), country_tier_record(country, continent))
    return writer


def main():
    city_writer = build_city_db()
    city_writer.to_db_file("edgion-test-city.mmdb")

    country_writer = build_country_db()
    country_writer.to_db_file("edgion-test-country.mmdb")

    print("Wrote edgion-test-city.mmdb and edgion-test-country.mmdb")


if __name__ == "__main__":
    main()
