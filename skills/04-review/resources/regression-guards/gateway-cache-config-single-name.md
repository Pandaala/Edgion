---
name: gateway-cache-config-single-name
description: Use when reviewing the Gateway process LRU cache configuration key.
knowledge_type: regression-guards
---

# Gateway LRU cache configuration has one key

## Conclusion and scope

The Gateway process YAML key is `lru_cache`. The `auth_cache` serde alias
mapped to the same field and had no distinct setting or supported release
contract.

## Rationale and evidence

`cli/config.rs` removed the alias; its fixture and the internal Gateway config
reference now use `lru_cache`. This does not change the cache implementation or
resolved limits.

## Verification and re-evaluation

Re-evaluate if the process config gets distinct auth-specific cache semantics.
The config parse fixture covers the canonical spelling.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-007`.
