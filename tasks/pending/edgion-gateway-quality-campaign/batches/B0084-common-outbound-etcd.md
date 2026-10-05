# B0084 — Shared outbound TLS and Etcd safety

## Scope and findings

Complete associated review covers grpc_tls, link_sys_core credential/health/client
factory/TLS, all Common Etcd provider files, their entire unit tests and the existing
HTTP2 header-limit integration/support files. Exact modified paths are the shared
outbound_tls/tls modules, Etcd client/config_mapping/ops and four unit test files.

Effective hostname/SAN constraints previously reached incapable rustls/reqwest/Etcd
builders without enforcement. They now reject the effective resolved policy,
including inheritance; local validation still replaces the global block whole.
Etcd init, reconnect and endpoint probe share fail-closed option construction;
errors cannot fall back to default unauthenticated/non-custom-TLS options.

Reqwest CA loading now uses the canonical complete parser. In addition to rejecting
a truncated trailing certificate, this enforces its existing per-bundle 1 MiB and
1,024-certificate bounds and rejects non-certificate content. Root replacement is
preserved. Namespace projection handles a stored key equal to its namespace as
the empty logical key; watch documentation now states inclusive revision semantics.

## Validation blocker

Shared tests: 109 passed, 4 pre-existing ignored. An initial new-test compile error
(unwrap_err required Client: Debug) was corrected with an explicit error branch.
Independent sync_review and watch_review found the implementation sound; workspace check and Clippy passed. Final
post-correction static review (including shutdown wording and D0021) passed.

The skill requires verification of affected configuration/lifecycle behavior.
No shared Gateway/Controller/Etcd topology is running, and docker ps fails because
/Users/caohao/.orbstack/run/docker.sock does not exist. Therefore required real
RedisTls, Etcd and Elasticsearch supported-path regression and unsupported-policy
recovery/reconnect verification cannot currently run. Keep ALL B0084 production
and test changes uncommitted. Unit construction tests do not establish handshake,
dynamic publication or reconnect/no-dial behavior. Do not restart shared services
or modify frozen edgion-tests to hide this gap.

D0021 records the independent Etcd unused-configuration design question. It is not
resolved by these safety fixes. Continue independently reviewable files.

The existing Etcd monitor knowledge page is corrected to distinguish cancellation
requests from joined completion and to reflect clone-before-RPC lock scope.
PublishedHealth deactivation is a reporter barrier, not a proof of no further
internal write to a retired client object. No shutdown redesign is included.

## Reviewed content fingerprints

Base: `99b85e31b`. SHA-256 identifies the reviewed working files (including
test and knowledge changes); runtime validation above applies to these contents.

```text
90f9f199efd99caab64f8d6954df33509fab2eb6cb910670cc65d045fedb2679  edgion-common/src/link_sys_core/outbound_tls.rs
321d48ad9738ad8ab5a7d4441bcb026910c7491351098ad8e7299056bd3b4673  edgion-common/src/link_sys_core/providers/etcd/client.rs
a057c3ac3f4e1953456b7bad84a019068908aa034f90cd561626551c61fc8703  edgion-common/src/link_sys_core/providers/etcd/config_mapping.rs
49d64c44765e59861c15e2a17bc266730627cff9f32251a4b7d0ac760166d8e5  edgion-common/src/link_sys_core/providers/etcd/ops.rs
9c798dcb81a13608fa6a033b6b850d11c55ac40bba987575fbd49ee83fbc960c  edgion-common/src/link_sys_core/tls.rs
27b81fae2b61ac841622bf04bf5798e5915a351c617f6fa6bbf5e4bb5615ce78  edgion-common/tests/unit/link_sys_core/providers/etcd/client_test.rs
436bdf9108f8cdebba69e326faeded8084f3293081b97caa1d6e525e99317f84  edgion-common/tests/unit/link_sys_core/providers/etcd/config_mapping_test.rs
b9df57bc3ff26c22c931b4492e23a19b9f655384aa8130b237e75f953f8b23c9  edgion-common/tests/unit/link_sys_core/providers/etcd/ops_test.rs
c4f08a00d4205fa59595dd61cd373610e45798d4c5df23e48a002d3b2ecd5d87  edgion-common/tests/unit/link_sys_core/tls_test.rs
f82e356f6520717e09d3025f0b34a2ac2f2951b13d003fc5b483235916a737a5  skills/04-review/concurrency/regression-guards/etcd-monitor-handle-not-aborted.md
```
