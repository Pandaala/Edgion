# Edgion Unit Testing

Use for these tasks:

- Writing unit tests for new or modified modules
- Improving code coverage
- Verifying the behavioral correctness of a single function / struct / module

## Design principles

- The combined goal of unit tests + integration tests: **maximize meaningful coverage without padding** — cover critical paths, branches, and risk boundaries with real assertions and push coverage as high as that allows; never add low-value tests just to hit a coverage number
- Unit tests cover: pure logic, data structures, parsing, validation, utility functions
- Integration tests cover: cross-process interaction, config sync, routing behavior, plugin runtime

## Project conventions

### Test location

The `edgion-common` pilot keeps unit-test bodies under `tests/unit/`, mirroring
their owning `src/` paths. Other crates are pending migration. Source files keep
only a test-gated module declaration, so the tests retain access to private items:

```rust
// In src/dns/httpdns.rs:
#[cfg(test)]
#[path = "../../tests/unit/dns/httpdns_test.rs"]
mod tests;
```

The file `tests/unit/dns/httpdns_test.rs` contains the module body, including
`use super::*`. Its nested location prevents Cargo from treating it as a
separate integration-test target. Files directly under `tests/` remain Cargo
integration targets.

For each migrated file, use `<crate>/tests/unit/<source directories>/<source stem>_test.rs`.
`src/dns/mod.rs` maps to `tests/unit/dns/mod_test.rs`. Preserve the original
test module name, feature and platform gates, and test identities. Multiple
separately gated modules may use files under `<stem>_test/`. Calculate `#[path]`
from the source file's directory and verify it with the compiler. Do not widen
production visibility to move tests. Benchmarks retain Cargo's `benches/`
location; `edgion-common` currently has none.

### Distribution of test files

Unit tests mainly cover the following areas:

| Area | Example path | Test focus |
|------|---------|---------|
| Resource type parsing | `edgion-resources/src/resources/*.rs` | CRD struct deserialization, field validation, preparse |
| Plugin configuration | `edgion-resources/src/resources/edgion_plugins/plugin_configs/*.rs` | `validate_and_init()`, default values, edge cases |
| LinkSys configuration | `edgion-resources/src/resources/link_sys/*.rs` | Connector config parsing and validation |
| Configuration system | `edgion-controller/src/config/mod.rs` | YAML parsing, config merging, default values |
| Gateway backends | `edgion-gateway/src/backends/mod.rs` | Backend selection, load-balancing logic |
| Working directory | `edgion-resources/src/work_dir.rs` | Path resolution, directory creation, permission checks |

## How to run

```bash
# Full unit test run
cargo test --all

# Run only one module's tests (select the owning workspace member)
cargo test -p edgion-resources --lib resources::http_route

# Run only a specific test function in its owning crate
cargo test -p edgion-gateway --lib test_function_name

# Show test output (for debugging)
cargo test --all -- --nocapture

# Sequential execution (avoid concurrency issues)
cargo test --all -- --test-threads=1
```

## Writing rules

### 1. Every new / modified public function should have a corresponding test

```rust
pub fn parse_duration(s: &str) -> Result<Duration, Error> { ... }

// In a migrated crate's tests/unit/<source stem>_test.rs:
use super::*;

#[test]
fn test_parse_duration_seconds() {
    assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
}

#[test]
fn test_parse_duration_invalid() {
    assert!(parse_duration("abc").is_err());
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
| `edgion-gateway/src/lb/runtime_state/mod.rs` leastconn count | P0-07 regression |
| `edgion-gateway/src/tls/validation/cert.rs` | mTLS verify_depth / SAN wildcard |
| `edgion-algo/src/cache/lru.rs` invariant tests | LRU underflow / hit_ratio |
| `edgion-gateway/src/routes/grpc/match_engine.rs` sort tests | spec conformance |
| `edgion-gateway/src/runtime/matching/route.rs` allowed_routes | spec conformance |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs` Condition mapping | reason/message text |
| `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs` port-conflict matrix | Protocol mixing rules |
| `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs` v1alpha3→v1 | Version compatibility |
| `edgion-resources/src/matcher/**` | IP / host matching algorithm |
| `edgion-resources/src/duration.rs` / `real_ip_extractor.rs` | Duration expression / header priority algorithm |
| `edgion-controller/src/authz/mod.rs` `Authorizer` admin-token merge / `cli_lookup` | Token authn + RBAC policy assembly |
| `edgion-resources/src/auth_dto.rs` `AuthStatusResponse` JSON wire format | CLI hard-parsing dependency |

### Reviewer checklist (mandatory for pruning PRs)

If a PR deletes any test in the table above, the description must state:
- **Reason** — why it can be removed
- **Replacement coverage** — which integration suite / which case provides equivalent coverage
- **Why integration is precisely equivalent** — integration tests are systemically blind to error-message text, internal invariants, and exception-path enumerations; prove this case isn't hit

## CI integration

Workspace tests run automatically in CI via `.github/workflows/ci.yml`:

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
