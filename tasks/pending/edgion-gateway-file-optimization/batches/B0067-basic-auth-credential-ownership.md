# B0067 — BasicAuth Credential Ownership and Unsupported Hashes

## Scope
- Baseline: `dec748cc`
- Pipeline state: independently reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/basic_auth/{htpasswd.rs,mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/basic_auth/plugin.rs`
- Expected no-change files: `edgion-gateway/src/plugins/http/basic_auth/{htpasswd.rs,mod.rs}`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, session traits/adapters, resource and Controller credential resolution, `htpasswd.rs`, auth common/runtime, metrics/access-log code, docs, other plugins, Cargo manifests, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation file; campaign ledger/changelog/main record; B0066 and B0067 batch records; reviewer-approved auth-wide deferred task records plus campaign backlinks; legacy follow-up index

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `dec748cc`; all three target blobs and relevant config/Controller/runtime/auth call chains remain unchanged.
- Baseline: BasicAuth focused tests pass 8/8.
- Sensitive ownership cost: construction clones the complete config including resolved users, then clones the resolved credential map again and retains both. A known-user request clones the complete user, stored hash, and decoded password before blocking verification.
- Unsupported-hash bug: bcrypt prefixes are not recognized as encoded, so the bcrypt text is SHA-wrapped as if it were plaintext; a client can authenticate by presenting the hash literal itself. The project contract is to reject unsupported bcrypt, not verify or reinterpret it.
- Decision: consume resolved users out of the retained runtime config, prepare immutable runtime users, and pin/relookup one map generation in `spawn_blocking` without password/hash String cloning. Recognized unsupported encodings remain unchanged and fail closed.
- Expected no-change files: `htpasswd.rs` remains the bounded canonical verifier for `{SHA}`, `$apr1$`, and `$1$`; `mod.rs` remains the minimal facade.
- Preserved behavior: plaintext preparation, supported hash bytes, `$5$`/`$6$` fail-at-verify behavior, unknown-user fast miss, shared failure delay, 401/body/challenge, anonymous mode, hideCredentials, upstream headers, exact PluginLog/group/user rules, sensitive username rendering, validation sealing, and immediate generation replacement on Secret update.

## Deferred candidates
- Borrowed/raw Authorization access is auth-wide because `PluginSession::header_value` allocates an owned String for affected Authorization-reading consumers; it spans the trait, adapter, mocks, and those auth plugins.
- A bounded credential-copy/lifetime audit is cross-crate and auth-wide because resolved and decoded credentials remain ordinary Strings through resources, Controller/conf-sync, runtime config, and sibling auth plugins. It must explicitly avoid promising end-to-end zeroization that the allocator and serialization stack cannot guarantee.
- Canonical auth identity-header sanitization must cover client spoofing, authenticated/anonymous transitions, invalid resolved values, and multiple-plugin order; its dependent future integration issue must observe the real upstream through new scenario files.
- Challenge realm quoted-string safety belongs to the shared auth response helper plus uniform config validation, not a BasicAuth-local escape.
- Existing integration tests remain frozen and already cover success, rejection, delay, grouping, hideCredentials, and hot reload. The bcrypt/hash-literal contract is deterministic in unit tests, so B0067 itself needs no new integration issue.

## Plan-first implementation
- Implementer: `b0055_runtime_analysis`
- Complexity: complex; changes sensitive-data ownership and blocking verification boundaries
- Required plan review: approved independently by `b0056_final_review` after a sensitive-memory correction
- Approved plan: consume resolved users out of the retained config; move prepared supported/unsupported hashes and groups into one immutable Arc map; consume the owned Authorization header, allocate username separately, and reuse only decoded storage as password; preserve unknown-user fast miss and pin/relookup one generation in `spawn_blocking` without password/hash cloning.
- Test contract: table supported and unsupported encodings, reject every unsupported hash literal, preserve plaintext SHA behavior and exact parsing/log errors, prove only valid moved buffers by pointer, retain config without credentials, and cover known/unknown and old/new pinned generations.
- Actual change: the retained config no longer holds resolved users; prepared supported/unsupported hashes and groups move into one immutable Arc map; owned Authorization is consumed before lookup/await with decoded storage retained only as password; known users verify inside one pinned generation without password/hash/full-user cloning; unsupported bcrypt/SHA-crypt stays exact and fails closed.
- Approved deviation: a private infallible `load_users` wrapper remains as the exact constructor-only cold-function key required by the gateway no-tracing registry; it delegates preparation once and emits only the existing count INFO.

## Independent review
- Plan reviewer: `b0056_final_review`
- Code reviewer: `b0063_fresh_code_review`, distinct from the plan reviewer
- Plan verdict: Approved after ensuring username spare capacity cannot retain password bytes; explicit drain consumption is required
- Code verdict: Approved with no findings; sensitive ownership, parsing, generation pinning, unsupported hashes, logs, headers, timing, and the cold wrapper were independently verified.
- Deferred adjudication: D0035–D0039 record the four nonduplicate auth-wide technical boundaries and the one required future identity-header integration scenario. Existing integration tests remain frozen; B0067 itself needs no integration issue.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline BasicAuth tests | passed | 8 tests |
| Implementation and focused tests | passed | 16 BasicAuth tests, 8 resource tests, 2 Controller resolver tests, and 34 PluginRuntime tests; BasicAuth passed ten repeated runs in review |
| Gateway all-target compile | passed | implementation and review verification |
| Independent code review | passed | fresh reviewer distinct from the plan reviewer; no findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | manifest unchanged; no existing integration test edited |

## Handoff
- Ledger/changelog updated: yes; one changed and two examined-no-change
- Commit: `d697aba1`
