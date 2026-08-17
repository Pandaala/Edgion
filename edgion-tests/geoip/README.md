# Synthetic GeoIP MMDB Test Fixtures

`edgion-test-city.mmdb` and `edgion-test-country.mmdb` are self-authored MaxMind
DB (MMDB) files used by the GeoIP country-restriction integration tests. They
are not derived from any MaxMind data.

## Why self-authored fixtures

Real MaxMind test databases only resolve public IP addresses, but this
repo's integration test clients always connect from loopback (`127.0.0.1`)
and RFC1918 addresses. A borrowed fixture would make most of the GeoIP
feature untestable from a closed harness. These fixtures map exactly the
loopback and RFC1918 ranges the integration tests use to synthetic
countries, so the entire decision table (allow/deny by country, tier
mismatch, dimension-absence, unmapped address) is reachable without any
outbound network access. Being self-authored, they also need no third-party
redistribution review.

## Address map (normative)

Every probe address used anywhere in the GeoIP country-restriction test plan
comes from this table. Do not edit it without updating every consumer.

| Network                | country | continent | city      | subdivision | coords |
|-------------------------|---------|-----------|-----------|-------------|--------|
| `127.0.0.1/32`          | CN      | AS        | Beijing   | BJ          | yes    |
| `127.0.0.5/32`          | RU      | EU        | Moscow    | MOW         | yes    |
| `127.0.0.6/32`          | US      | NA        | (none)    | (none)      | no     |
| `127.0.0.7/32`          | HK      | AS        | Kowloon   | KOW         | yes    |
| `10.0.0.0/8`            | US      | NA        | Cambridge | MA          | yes    |
| `203.0.113.0/24`        | GB      | EU        | London    | ENG         | yes    |
| `198.51.100.0/24`       | HK      | AS        | (none)    | (none)      | no     |
| anything else (incl. `8.8.8.8`, `127.0.0.2-4`) | *unmapped* | | | | |

- `127.0.0.6` and `198.51.100.0/24` are country-only **on purpose** in the
  City fixture: they exercise the per-record dimension-absence path (a
  record that simply lacks city/subdivision/location fields) even when the
  *database* generally does carry that data — the same ordinary "missing
  value" outcome as loading a database that never carries city data at all.
  There is no separate "database tier" concept; both cases resolve to
  `city: None` and are governed by the rule's `onMissing`.
- `8.8.8.8` is the `GeoLookup::NotFound` probe.
- **Deliberately no record at `::ffff:0:0/96`.** Both fixtures use
  `ipv4_compatible=True`, which stores IPv4 networks under the `::/96`
  "compatible" prefix, never the `::ffff:0:0/96` "mapped" prefix. The
  absence of a mapped-prefix record reproduces the exact IPv4-mapped-address
  pathology the design calls out (design §4.1): a reader that fails to
  normalize an incoming `::ffff:x.x.x.x` address down to plain IPv4 before
  lookup will find nothing, making `normalize_addr`'s handling a falsifiable
  assertion instead of a vacuous one.

### Country fixture content

`edgion-test-country.mmdb` carries only `country` + `continent` for every
mapped network above (a true Country-tier schema has no city-level fields at
all, so there is no country-only-vs-full distinction there — every entry is
country-only).

### City fixture content

`edgion-test-city.mmdb` carries the full `country` + `continent` + `city` +
`subdivision` + `location` (latitude/longitude + `accuracy_radius: 100`) for
every network above **except** `127.0.0.6/32` and `198.51.100.0/24`, which
carry only `country` + `continent` (see dimension-absence note above).

## `database_type` predicate (pinned)

The gateway's tier check (`check_database_tier`, Task 10 step 6) decides
"is this a City-tier database" via `database_type.contains("City")`. These
two strings must never drift apart:

- City fixture: `database_type = "Edgion-Test-City"` (contains `"City"`)
- Country fixture: `database_type = "Edgion-Test-Country"` (contains
  `"Country"`, does **not** contain `"City"`)

## Reproducibility

`build_epoch` is pinned to a fixed constant (`1755043200`,
2026-08-13T00:00:00Z, an arbitrary fixed point with no other significance)
via a `time.time` monkeypatch in `generate.py`, because `mmdb_writer`'s
`MMDBWriter._build_meta()` hardcodes `int(time.time())` with no override
hook. Re-running `generate.py` therefore produces byte-identical output.

## Regenerating the fixtures

```bash
python3 -m venv /tmp/mmdbgen
/tmp/mmdbgen/bin/pip install mmdb_writer==0.2.7 netaddr maxminddb
cd edgion-tests/geoip
/tmp/mmdbgen/bin/python3 generate.py
```

`mmdb_writer` requires Python >= 3.9. The host Python at the time these
fixtures were generated was 3.9.6 (exactly the floor `mmdb_writer` declares).
`mmdb_writer` 0.2.7 (the latest release at generation time) installs and
runs correctly on 3.9.6, so no host Python upgrade was needed and no older
release had to be pinned.

This toolchain is not part of CI or the build path — the same arrangement as
`edgion-tests/wasm/build.sh`, which needs a wasm target nobody installs in
CI. The generated `.mmdb` files are committed directly; the venv and
`mmdb_writer` are throwaway generation-time tooling.

## Verification performed

1. **Python `maxminddb` reader**: every row of the address map above,
   including `8.8.8.8` and `::ffff:127.0.0.1` resolving to nothing, was
   confirmed against both fixture files.
2. **Rust `maxminddb` crate**: an integration test
   (`edgion-gateway/tests/geoip_fixture_readback.rs`, kept permanently as a
   fixture-integrity regression test) opens both fixtures via
   `maxminddb::Reader::open_mmap` and decodes every probe
   through `geoip2::City` / `geoip2::Country`, asserting country ISO,
   continent code, city name, subdivision ISO, and latitude/longitude all
   match the address map exactly. This step matters because a field-type
   mismatch between the Python writer and the Rust reader's expected type
   (for example `geoip2::City`'s `location.latitude: Option<f64>`) fails
   *silently* — a mismatched type decodes to `None` instead of erroring, so
   downstream coordinate assertions would otherwise pass vacuously. No
   mismatch was found; `geoip2::City`'s `f64` latitude/longitude fields
   matched `mmdb_writer`'s default `float_type="f64"` without any generator
   changes needed.
