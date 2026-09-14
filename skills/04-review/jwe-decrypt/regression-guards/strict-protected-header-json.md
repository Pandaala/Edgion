---
name: jwe-strict-protected-header-json
description: Use when reviewing compact JWE protected-header parsing, duplicate JSON members, JOSE parameter extraction, or AES-GCM AAD construction.
knowledge_type: regression-guards
---

# JWE protected headers use strict object parsing and raw AAD

## Conclusion and scope

`JweDecrypt::parse_compact_jwe` accepts a compact-JWE protected header only when
its decoded JSON is an object and every decoded member name is unique. Duplicate
known members such as `alg` or `enc` and duplicate unknown members are invalid
format errors. Arrays, strings, numbers, booleans, and `null` are also invalid
formats rather than missing-header errors.

Unknown non-critical members remain accepted. The exact encoded protected segment,
not parsed or reserialized JSON, remains the AES-GCM additional authenticated data.
This rule applies to the Gateway request-time JWE parser and does not change the
operator-facing `JweDecryptConfig` schema.

## Rationale and evidence

The former `serde_json::Value` path accepted every JSON top-level shape and lost
duplicate-member information while constructing its map. Subsequent `get("alg")`
and `get("enc")` calls therefore classified non-objects as missing headers and
could interpret an ambiguous duplicate-member object.

The module-private `ProtectedHeader` deserializer now requires a map and records
all decoded member names during its single traversal. It retains only the `alg`
and `enc` values plus `zip` and `crit` presence; other values are consumed with
`IgnoredAny`. `ParsedJwe::protected_segment` still flows unchanged into
`decrypt_in_place_detached` as AAD.

## Alternatives and limits

Deserializing into a normal derived struct is insufficient because it can reject
duplicate declared fields but cannot detect duplicate unknown fields while still
allowing unique unknown non-critical parameters. Parsing to a generic JSON value
before validation is also insufficient because duplicate names have already been
collapsed.

This correction prevents parser ambiguity inside Edgion. It does not by itself
establish that the former behavior enabled an authentication bypass: a successful
token still required a valid AES-GCM tag over the original protected segment.
Algorithm selection, the configured allowlist, token-size cap, key-failure
precedence, response mappings, and static PluginLog values remain unchanged.

## Verification and re-evaluation

`cargo test -p edgion-gateway --lib plugins::http::jwe_decrypt::plugin::tests`
passed all 15 focused tests after the correction. The tests cover duplicate `alg`,
duplicate `enc`, duplicate unknown members, every non-object JSON shape, `zip` and
`crit` presence, a unique unknown non-critical member, and successful decryption
using the original encoded protected segment as AAD.

Re-evaluate this rule if Edgion adopts a shared JOSE parser, adds protected-header
extension processing, or changes from compact JWE to a representation whose
protected-header or AAD construction rules differ.

## Resolution references

- `harden-jwe-protected-header-json-parsing` — corrected as a Gateway parsing
  regression guard in September 2026.
