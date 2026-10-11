---
id: ARCH-083
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-428
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Pass the current federation snapshot to each supervised client start

The federation supervisor detects an address or identity change and restarts the client, but the production spawn closure always supplies the original startup configuration. The restart therefore cannot establish the connection that the supervisor's current snapshot requests.

`FederationSupervisor` in `edgion-controller/src/fed_sync/supervisor.rs:44–75` accepts a spawn closure whose only argument is the shutdown receiver. `apply` checks the supplied `CenterClientConfig` to decide whether to run, but `start` does not pass that snapshot to the closure. `run_supervisor:249–264` explicitly compares address, name and cluster, stops the old client, and applies the new snapshot. Its reload contract states that this restart uses the updated identity/address.

The actual caller in `edgion-controller/src/cli/mod.rs:767–774` captures `center_config` from startup and clones it inside every spawn. `fed_client::run` receives that owned config; `connect_and_run` derives the endpoint from its address and constructs the registration identity, cluster, environment and tags from it. The supervisor's newer snapshot never reaches those consumers.

For example, start a leader Controller with an enabled, valid federation configuration pointing to address A. Change only the address to B through the existing config reload path. `apply_center_reload` loads and sends the new Center snapshot after applying the policy/token update. The supervisor sees A→B, waits for the old task to stop, and spawns a new one. The closure still passes A, so the new client reconnects to A. The supervisor then remembers B; another identical notification is no longer an identity edge. A follower that receives B before promotion likewise starts from A when later promoted. These are source-supported scenarios, not claims of an observed deployment incident.

Make the current immutable `CenterClientConfig` an explicit input to the spawn operation. Pass the initial snapshot at the first start, the newly applied snapshot after an identity restart, and the remembered latest snapshot on leadership promotion. The production closure should clone only shared router/ConfMgr/Authorizer dependencies and consume that explicit snapshot. Avoid rereading the watch channel inside the closure or adding another mutable configuration store: either would introduce an unnecessary second source of lifecycle truth.

Preserve the current boundaries: federation is instantiated only when the Center section is present at boot; enabled-and-leader determines whether it runs; stop remains awaited with the existing bounded hung-task exception; intermediate watch values may coalesce; the reload handler remains the single policy writer; policy-only changes keep the existing connection and use the live Authorizer. This finding does not request a new restart trigger for security/environment/tag fields, live TLS policy, automatic startup when the Center section was absent, or a change to absent-section reload semantics. Passing the effective snapshot on an already required start is sufficient.

Existing supervisor tests exercise start/stop counts, leadership/config edges and a fake client's shutdown receiver. They do not assert the actual configuration delivered to a spawned client, which is where the production closure loses the snapshot. Extend that boundary test to capture received snapshots for an address restart and follower-to-leader promotion, while retaining the policy-only no-restart test. No tests or network connections were run during this review.

The failure is independent of legacy ConfSync TLS-owner retirement (ARCH-074), installed-leaf validation (ARCH-075), and the Controller skip-TLS precedence issue (ARCH-080): even with unchanged valid TLS and policy, the explicitly requested new address is discarded.
