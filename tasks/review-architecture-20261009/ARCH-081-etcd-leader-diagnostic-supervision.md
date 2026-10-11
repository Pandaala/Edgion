---
id: ARCH-081
status: candidate
kind: behavior-defect
priority: P2
partition: PART-395
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-081: Keep leader diagnostics outside the post-win supervision boundary

## Current ownership and evidence

`edgion-controller/src/conf_mgr/conf_center/etcd/leader_election.rs:172-202::one_term` publishes a fresh term token, records freshness and sets `is_leader=true`, then awaits `refresh_current_leader()` at line 178. Only after that helper returns does it spawn the authoritative leader-key guard and enter `supervise_term`. Demotion, local flag clearing and term-token cancellation occur after the supervisor returns.

`refresh_current_leader` at lines 401-409 builds a dedicated election session and awaits its Leader RPC. Neither operation is selected against shutdown or keepalive-driver completion, and the helper has no explicit time bound. Common `client.rs:431-442` uses retained election options; `config_mapping.rs:186-200` deliberately omits their unary request timeout. This is necessary for blocking campaign semantics. Controller bootstrap supplies a bounded dial and HTTP/2 keepalive, but these are different SDK options from a per-request deadline. Pinned registry `etcd-client` 0.18.0 maps connect timeout and request timeout separately in `src/client.rs:221-234`; its `src/rpc/election.rs:532-539` directly awaits Leader. A responsive HTTP/2 transport does not establish a response deadline for that unary RPC.

The actual active-passive lifecycle in `non_kube_common.rs:221-236,377-388` starts and detects loss through `LeaderHandle::is_leader`. It does not compose that flag with lease freshness. The stale-write `LeadershipGate` remains a separate defense, as explicitly demonstrated by `gate_blocks_when_lease_stale_even_if_flag_set`. The gRPC server captures the published term token for Watch and WatchServerMeta; cancellation of that token belongs to demotion.

## Failure scenario and root cause

After campaign returns a winning key, the post-win Leader lookup can remain pending while its transport remains connected. If the keepalive driver then exits or stops receiving successful renewals, its JoinSet completion and the renewal deadline cannot reach the supervisor because `one_term` is still awaiting the diagnostic lookup. The authoritative guard has not been started either. The local leader flag and term token remain published; shutdown is also not observed at this await. Once freshness expires, the local write gate can reject new writes while the active-passive lifecycle still sees the leader flag as true. This is a source-level failure scenario, not a reproduced service incident or a claim that every network outage triggers it.

The root cause is a best-effort identity lookup in the sequential path between public authority publication and authoritative supervision. Making `supervise_term` itself free of Etcd IO does not protect the term while this preceding await is pending.

## Bounded improvement and alternative

Make the winner transition enter guard and renewal/shutdown supervision without waiting for diagnostic network IO. One bounded option is to retain the typed `LeaderInfo` already constructed cold by `new`, serialize that same value for campaign, and publish its known identity locally after the acknowledged win. Keep the existing bounded, candidacy-owned network refresher for discovering another leader. Retain the exact resolved identity used for campaign; do not re-read environment state to produce a potentially different diagnostic identity.

If a post-win server lookup remains required, run it as an owned, explicitly bounded and cancellable diagnostic operation while the term supervisor is already active. Diagnostic failure must not become authoritative lease loss, and its owner must terminate it on demotion/shutdown. Merely moving the await after guard spawn still blocks renewal supervision; merely adding a channel-wide unary timeout would break campaign semantics. Choose the smallest local ordering/ownership change, without a generic election framework or SDK-wide timeout policy.

## Preserved contracts and future verification

Preserve publication of the fresh term token before the leader flag, keepalive during candidacy, dedicated campaign connections without unary deadlines, authoritative leader-key comparisons, renewal and pause detection, first-step local demotion and term-token cancellation, bounded best-effort resign/revoke, and best-effort follower identity reporting. Preserve the accepted local-at-transaction write-gate boundary rather than adding election-key fencing to writes. Do not change K8s AllServe behavior, workqueues, schema fields, TLS/credential preparation, native SDK ownership or request observability.

The complete current `leader_election_test.rs` was read. It covers normal single-candidate readiness, mutual exclusion/takeover, overwritten-key demotion, release/reacquire and leaseless sweeping. `candidate_survives_and_wins` explicitly waits for identity publication after the flag flips, but none of these tests holds the post-win Leader RPC pending. Future distinguishing coverage should hold only that diagnostic operation pending and prove that shutdown, authoritative driver loss or the renewal deadline still clears the flag and cancels the published term token; normal identity/takeover tests must remain valid. If the RPC is removed from the winner transition, verify the diagnostic identity equals the same cold value submitted by campaign. No test/service execution occurred during review.

## Original decisions, deduplication and limits

The current original Etcd SDK boundary and ConfCenter bootstrap pages require dedicated no-unary-timeout election options and permit native SDK behavior; they do not authorize unbounded diagnostic work to delay the Controller's supervision. The accepted `etcd-leadership-gate-not-fenced-at-txn.md` concerns a process-local check before an already-started store transaction, not failure to start term supervision. The K8s per-write versus AllServe lifecycle decision concerns a different mode. No overlapping diagnostic-supervision finding was found in current review artifacts or routed knowledge.

Source comments cite an original Etcd leader-election specification that is absent from the current tree. The missing historical document is not treated as evidence of acceptance. The failure claim follows current source, actual lifecycle consumers, pinned dependency behavior and read test bodies. Initial capability probing, network calls during resignation, global lifecycle cleanup, arbitrary server payload handling and broader SDK transport guarantees are outside this candidate.
