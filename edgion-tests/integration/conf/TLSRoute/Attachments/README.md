# TLSRoute attachment hostname regression

This local integration scenario runs against the shared Controller, Gateway,
and `test_server`. Load `bootstrap/` before Gateway startup: listener sockets are
bound at startup, while the scenario changes configuration without a restart.
The ordinary broad configuration loader also discovers this directory.

Three logical Gateways share two physical TLS passthrough sockets:

| Gateway | Port | Main listener hostname |
| --- | --- | --- |
| `tls-attachments-a` | 31975 | `a.attachments.example.com` |
| `tls-attachments-b` | 31976 | `b.attachments.example.com` |
| `tls-attachments-c` | 31975 | `b.attachments.example.com` |

The older target Route declares both `a` and `b`, attaches only to A and B,
and uses HTTPS backend 30051. The newer competitor attaches to C and uses mTLS
backend 30052. Requests for `b` on port 31975 must reach 30052, even while the
older target exists. This catches invalid attachment candidates shadowing a
valid Route; a closed connection does not pass.

A also has an exact listener that narrows a wildcard Route. B has an
unrestricted listener, which preserves that wildcard and the explicitly
declared hostnames of another Route. Negative probes cover wildcard siblings
on A, the wildcard root, and undeclared hostnames on B. All Routes specify
nonempty Gateway API v1 `hostnames`; no hostname-less Route catch-all is used.

`TlsRouteAttachments` applies and polls these phases:

1. Remove any preloaded target and verify absence in Controller and Gateway.
2. Apply the initial Route and verify the exact serving backend identities.
3. Publish a Gateway-wide parent, wait for it in both resource caches, rename
   A's main listener without changing its hostname/port, and verify the new
   attachment name plus unchanged backend identity before restoring.
4. Remove A from `parentRefs`, verify A stops serving the target, and restore.
5. Remove `b` from Route `hostnames`, verify B stops serving it, and restore.
6. Change A's listener hostname, verify dependent Route reprocessing, and restore.
7. Delete and recreate the target, then clean it up.

Every phase probes independent healthy controls on both sockets and the
competitor on C. Failures and success both restore Gateway A and delete the
target. Bootstrap Gateways, Services, EndpointSlices, and baseline Routes
remain available to the shared topology. Synchronization uses `wait_phase`;
timeouts and connect failures never count as successful rejection.

The existing HTTPS backends return `Server: 0.0.0.0:<port>` from `/echo`.
The client checks that exact response identity, not merely handshake success.
It reuses `edgion-tests/certs/mtls/valid-client.crt` and `valid-client.key` for
backend 30052; `start_all_with_conf.sh` already supplies its trusted client CA.
These fixtures use loopback EndpointSlices and have no Kubernetes mirror.

After loading bootstrap fixtures and starting the shared topology, run from
the repository root:

```bash
./target/debug/test_client -g --dynamic-scenario TlsRouteAttachments
```

Run this before the shared topology's final `cache_residue` wipe-all. Reloading
Gateways after that wipe hits the separately tracked
[TLS listener manager recreation defect](../../../../../tasks/todo/tls-listener-recreation-stale-route-manager.md).

The scenario does not claim full Gateway API conformance or exercise independent
same-socket Listener specificity when two valid Listener attachments overlap.
