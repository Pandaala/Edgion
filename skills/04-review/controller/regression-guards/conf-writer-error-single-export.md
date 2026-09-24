---
name: conf-writer-error-single-export
description: Use when reviewing Controller configuration-writer error exports.
knowledge_type: regression-guards
---

# Configuration writer error has one export

## Conclusion and scope

`conf_mgr` exports the writer error as `ConfWriterError`. Its former
`ConfStoreError` alias had no in-repository consumer or distinct behavior.

## Rationale and evidence

The duplicate public alias in `conf_mgr/mod.rs` was removed under the
repository's pre-release no-compatibility rule. Search for the old name and
compile the Controller when changing this interface.

## Verification and re-evaluation

Re-evaluate if a new supported consumer requires a distinct error type;
adding another name alone would not establish a distinct contract.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-003`.
