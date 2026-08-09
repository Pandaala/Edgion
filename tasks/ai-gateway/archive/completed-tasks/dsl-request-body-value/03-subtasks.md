# Implementation subtasks

## Ownership rules

Subtasks run in dependency order when they touch an interface. No two agents
may edit the same file concurrently. An agent must report an unworkable design
instead of adding a materializing fallback or changing public semantics.

## A. DSL core contract

Owned files:

- workspace `Cargo.toml` and `edgion-dsl/Cargo.toml` only for the direct regex
  dependency
- `edgion-dsl/src/value.rs`
- `edgion-dsl/src/bytecode.rs`
- `edgion-dsl/src/compiler/`
- `edgion-dsl/src/validator.rs`
- a new definite JSON-path parser module under `edgion-dsl/src/`

Deliverables:

- opaque request Body value within the 24-byte layout
- bytecode version bump and defensive validation
- method lowering and compile-time path/streaming-regex validation
- bytecode introspection that distinguishes Body reads from mutations for
  stage-aware validation
- fallible implicit string coercion that rejects Body
- DSL-core unit tests

Out of bounds:

- gateway runtime, request storage, resources schema, and user documentation

## B. Request-body source and mutation backend

Owned files:

- `edgion-gateway/src/request_body/`
- directly related request-body storage tests in those files

Deliverables:

- flavor-agnostic operation facade with stock behavior
- immutable source-range snapshots
- one-job positional disk scanning with the 32-permit limiter and cooperative
  cancellation
- streaming lossy UTF-8 text scanners and actual-byte accounting
- transactional append/prepend/remove/literal replace
- replay of stable snapshots without materialization
- memory, spill, boundary, cancellation, limit, rollback, and retry tests

Out of bounds:

- PluginSession, DSL VM/compiler, resources schema, and integration fixtures

## C. Session and VM integration

Starts only after A and B interfaces are stable.

Owned files:

- `edgion-gateway/src/plugins/runtime/traits/session.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/plugins/http/dsl/`
- `edgion-gateway/Cargo.toml` only for the direct streaming-regex dependency
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs`
- `edgion-resources/src/resources/edgion_plugins/validate.rs` only for
  stage-aware DSL validation
- directly related unit tests in those modules

Deliverables:

- session delegation to the request-body facade
- capture-only DSL body-gate capability
- VM scan budget and async shared text dispatch
- fixed fail-close body-access error handling
- selective JSON conversion and mutation calls
- read-only late request-body access in upstream response filters and
  request-stage-only mutation validation
- rejection of every implicit Body-to-string path

Out of bounds:

- request storage internals and unrelated plugin refactors

## D. Documentation and integration

Starts after C passes targeted unit tests.

Owned files:

- DSL user documentation
- relevant `skills/` architecture and error-semantics documents
- `edgion-tests/integration/conf/EdgionPlugins/DslBody/`
- `edgion-tests/src/client/suites/edgion_plugins/dsl/body.rs`
- task implementation and decision records

Deliverables:

- complete API/migration documentation
- memory and forced-spill external behavior scenarios
- stock build and repository verification record
