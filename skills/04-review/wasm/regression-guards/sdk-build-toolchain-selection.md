---
name: wasm-sdk-build-toolchain-selection
description: Use when changing optional Go or TinyGo compatibility fixture build tooling.
knowledge_type: regression-guards
---

# Go and TinyGo share the explicitly selected SDK

## Conclusion and scope

build-compatibility.sh treats GO_BIN as the Go SDK selector for both the Go
reactor and TinyGo probes. It resolves executable paths before changing directory,
queries GOROOT without the inherited root, disables automatic toolchain selection
with GOTOOLCHAIN=local, and passes the resulting GOROOT and bin path to TinyGo.
A mismatched TinyGo root fails before any fixture build.

## Rationale and evidence

TinyGo does not consume GO_BIN. Merely using that variable for go mod verify
leaves TinyGo compiling against an unrelated system SDK, even when the user
supplies the pinned Go executable. Inherited GOROOT and GOTOOLCHAIN can likewise
invalidate the selected toolchain's provenance.

The isolated script regression in
cicd/checks/tests/wasm_compatibility_toolchain_test.py covers relative paths with
spaces, PATH-resolved names, conflicting inherited settings and early rejection
of a wrong TinyGo root. It uses temporary fake tools without rewriting artifacts.

## Limits and re-evaluation

This does not install compilers, promise support for arbitrary versions, or add
WASI runtime support. Selected versions must still meet the SDK/compiler contract.
Recheck this rule when Go or TinyGo changes SDK selection; fixture hashes and
static compatibility are separate from toolchain identity.
