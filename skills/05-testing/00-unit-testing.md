# Edgion Unit Testing

Use for these tasks:

- Writing unit tests for new or modified modules
- Improving code coverage
- Verifying the behavioral correctness of a single function / struct / module

## Design principles

- The combined goal of unit tests + integration tests: **99% code coverage**
- Unit tests cover: pure logic, data structures, parsing, validation, utility functions
- Integration tests cover: cross-process interaction, config sync, routing behavior, plugin runtime

## Project conventions

### Test location

Edgion uses Rust's standard **inline test module** pattern; tests live at the bottom of the source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        // ...
    }
}
```

A top-level `tests/` directory (crate-level integration tests) is **not** used for unit tests: those files compile as a separate crate and can only reach the public API, so they cannot test private items.

### Separating large test blocks (when a file is dominated by tests)

When the inline test block makes a file hard to read/review, move the tests into a sibling **in-crate** test file. **Trigger** (guideline, not a hard gate): test lines > ~60% of the file **and** file > ~800 lines. Below that, keep tests inline — selectivity is what keeps the tree from filling with test files.

The mechanism keeps the test module a **child of the source file** (via `#[path]`), so it retains full access to module-private items — no visibility changes are ever needed.

**1. In the source file** `foo.rs`, replace the whole `#[cfg(test)] mod tests { ... }` block with this footer at the very bottom:

```rust
// Unit tests for this module live in `tests/foo_test.rs`.
#[cfg(test)]
#[path = "tests/foo_test.rs"]
mod tests;
```

**2. The test file** `<source_dir>/tests/foo_test.rs` starts with a back-pointer header, then the moved body:

```rust
//! Unit tests for `foo.rs` (see `super`).
use super::*;
// ... the moved test body, verbatim ...
```

**Rules:**
- Naming: `<source_stem>_test.rs` (singular `_test`). For a directory module whose code is in `mod.rs`, name it after the directory (`config/tests/config_test.rs`).
- The `tests/` sub-dir needs **no** `mod.rs` — each file is pulled in only by its source file's `#[path]`. Multiple source files in one directory share one `tests/` dir (each is a distinct child module, no collision).
- **Never** bump an item's visibility (`pub`/`pub(crate)`) just to relocate a test. If the move needs that, you are using the wrong mechanism — go back to `#[path]`.
- Pure move: same test count in and out; no logic edits. Verify with `cargo test --lib <module>::tests` before/after.
- `#[path]` resolves relative to the **directory of the source file**, so `#[path = "tests/foo_test.rs"]` written in `bar/foo.rs` loads `bar/tests/foo_test.rs`.

**Finding separated tests (grep standard for AI/humans):**

```bash
# List every source file that has an externalized test file:
rg -l '#\[path = "tests/.*_test\.rs"\]' src
# Review only production changes (exclude separated tests + the integration harness):
git diff main... -- ':(exclude)**/tests/*_test.rs' ':(exclude)**/tests.rs' ':(exclude)examples/code/**'
```

The leading `// Unit tests for this module live in ...` comment + the `#[path]` line are the machine-readable markers; AI review tooling should scope to non-test paths via the exclusion above.

### Distribution of test files

Unit tests mainly cover the following areas:

| Area | Example path | Test focus |
|------|---------|---------|
| Resource type parsing | `src/types/resources/*.rs` | CRD struct deserialization, field validation, preparse |
| Plugin configuration | `src/types/resources/edgion_plugins/plugin_configs/*.rs` | `validate_and_init()`, default values, edge cases |
| LinkSys configuration | `src/types/resources/link_sys/*.rs` | Connector config parsing and validation |
| Core utility functions | `src/core/common/utils/mod.rs` | String handling, path utilities, common logic |
| Configuration system | `src/core/common/config/mod.rs` | TOML parsing, config merging, default values |
| Gateway backends | `src/core/gateway/backends/mod.rs` | Backend selection, load-balancing logic |
| Working directory | `src/types/work_dir.rs` | Path resolution, directory creation, permission checks |

## How to run

```bash
# Full unit test run
cargo test --all

# Run only one module's tests
cargo test --lib -p edgion -- types::resources::http_route

# Run only a specific test function
cargo test --lib test_function_name

# Show test output (for debugging)
cargo test --all -- --nocapture

# Sequential execution (avoid concurrency issues)
cargo test --all -- --test-threads=1
```

## Writing rules

### 1. Every new / modified public function should have a corresponding test

```rust
pub fn parse_duration(s: &str) -> Result<Duration, Error> { ... }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("abc").is_err());
    }
}
```

### 2. Use `tempfile` for filesystem tests

```rust
#[test]
fn test_work_dir_creation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let work_dir = WorkDir::new(tmp.path().to_path_buf());
    assert!(work_dir.ensure_dirs().is_ok());
}
```

### 3. Test names should clearly express intent

```rust
#[test]
fn test_cors_config_default_allows_all_origins() { ... }

#[test]
fn test_cors_config_rejects_empty_allow_methods() { ... }

#[test]
fn test_reference_grant_matches_exact_namespace() { ... }
```

### 4. Test boundary conditions and error paths

Each `validate_and_init()` should at minimum cover:
- Valid default configuration
- Missing required fields
- Invalid values (empty string, out-of-range number, format error)
- Boundary values

### 5. Avoid antipatterns in tests

- **Do not** mock the database or external services — that belongs to integration tests
- **Do not** start HTTP servers or gRPC inside unit tests — use integration tests
- **Do not** depend on absolute filesystem paths — use `tempfile`
- **Do not** write empty tests that only verify "no panic"

## Must-read before deleting / merging unit tests: decision rules

When pruning / merging unit tests (parameterizing, removing tests covered equivalently by integration), **a test matching any of the following must be kept as a unit test** — do not delete and do not merge it to the point of losing granularity:

1. **Data-structure invariants** — e.g., LRU counter underflow, NaN protection when `hit_ratio` denominator is 0
2. **Error message text** — mapping from `reason` / `message` of a Condition to an enum (integration tests cannot precisely compare text)
3. **Algorithmic correctness** — IP CIDR validity, radix-tree construction, hostname wildcard coverage
4. **Spec compliance** — sort/filter behavior required by the Gateway API spec (e.g., regex-length priority)
5. **Wire format / serialization compatibility** — serde attributes like `rename_all` (fields hard-parsed across CLI / processes)
6. **Security boundaries** — mTLS `verify_depth` range, cipher allowlist, SAN coverage logic
7. **Version compatibility** — CRD version conversion such as v1alpha3→v1 (integration usually runs only the latest version)
8. **Regression fixes** — tests with a historical bug ID (e.g., P0-07 leastconn count)
9. **Pure cryptography / JWT issuance and verification** — cannot be proved by black-box requests

### Untouchable list (examples, not exhaustive)

| Location | Reason |
|---|---|
| `src/core/gateway/lb/runtime_state/mod.rs` leastconn count | P0-07 regression |
| `src/core/gateway/tls/validation/cert.rs` | mTLS verify_depth / SAN wildcard |
| `src/core/gateway/cache/lru.rs` invariant tests | LRU underflow / hit_ratio |
| `src/core/gateway/routes/grpc/match_engine.rs` sort tests | spec conformance |
| `src/core/gateway/runtime/matching/route.rs` allowed_routes | spec conformance |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` Condition mapping | reason/message text |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs` port-conflict matrix | Protocol mixing rules |
| `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` v1alpha3→v1 | Version compatibility |
| `src/core/common/matcher/**` | IP / host matching algorithm |
| `src/core/common/utils/duration.rs` / `real_ip_extractor.rs` | Duration expression / header priority algorithm |
| `src/core/common/unified_auth/mod.rs` `from_configs` / `require_auth` | Multi-source auth assembly |
| `src/core/common/local_auth/handlers.rs` `AuthStatus` JSON wire format | CLI hard-parsing dependency |

### Reviewer checklist (mandatory for pruning PRs)

If a PR deletes any test in the table above, the description must state:
- **Reason** — why it can be removed
- **Replacement coverage** — which integration suite / which case provides equivalent coverage
- **Why integration is precisely equivalent** — integration tests are systemically blind to error-message text, internal invariants, and exception-path enumerations; prove this case isn't hit

## CI integration

Unit tests run automatically in CI via `.github/workflows/ci.yml`:

```yaml
- name: Run tests
  run: cargo test --all
```

Triggered on every push to `main`/`master` and every PR.

## Division of labor with integration tests

| Dimension | Unit test | Integration test |
|------|---------|---------|
| **Scope** | Single function / module | Controller + Gateway + test_server cross-process |
| **Speed** | seconds | minutes |
| **External deps** | None (at most tempfile) | Multiple processes must be started |
| **Coverage focus** | Parsing, validation, pure logic | Config sync, routing, plugin runtime, TLS |
| **Run command** | `cargo test --all` | `run_integration.sh` |
| **Failure means** | Bug in one function's logic | Cross-component interaction issue |
