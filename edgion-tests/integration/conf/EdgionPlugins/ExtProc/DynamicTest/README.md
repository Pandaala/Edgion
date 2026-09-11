# ExtProc TLS policy recovery

`ExtProcTlsPolicyRecovery` uses the existing shared Controller/Gateway topology
and the full-tier independent Go processor. The scenario renders the two
`.yaml.tmpl` files in memory with unique resource names and the managed fixture's
TLS target. Static suite loading never applies these templates.

Both `failureModeAllow` values run through the same phases:

1. A supported private-CA policy completes TLS and both header phases.
2. Local hostname override and nonempty SAN constraints seal generic HTTP 500
   rejection, with no additional TCP accepts or RPCs.
3. Removing the unsupported local policy restores successful processing.
4. A dedicated global default with a CA source and unsupported hostname/SAN is
   published before a new plugin generation with no local validation block.
   These inherited policies also reject without dialing or making RPCs.
5. A local CA block with an empty SAN list replaces the unsupported global
   validation block and restores processing.
6. A supported global CA block and a newly published inheriting plugin recover.

The Controller's Accepted status is checked separately: local unsupported
fields are advisory errors, while inherited fields are enforced by the Gateway
constructor. Publication polling checks phase markers and actual policy fields,
then waits for new per-resource runtime generation IDs in the existing debug
store inventory. Raw ConfigClient arrivals alone precede debounced runtime
publication and are insufficient readiness evidence. Initial route-table counts
and scoped route inventory prove the routes are published before traffic.
Each inherited phase deletes and recreates only its dedicated global config,
waiting for the runtime config count to drop and rise; the count changes after
global TLS recomputation. Processor polling checks terminal TLS stream
observations. Negative counters span global/plugin publication and traffic
probes, and are never reset or retried away.
The fixture counts TCP accepts before the TLS handshake and RPCs independently
of the request's test headers, including RPCs on reused connections.

Global aggregation currently adopts validation only from a unique CA-declaring
configuration. The scenario therefore declares a real CA reference. It does not
claim coverage of identity-only global aggregation or global-only invalidation
of already compiled plugins: each inherited phase publishes a new plugin entry.
Shared-topology scenarios run serially; no other traffic may target the fixture
during the counter assertions. The scenario deletes only its own two routes,
two plugins, and global config on success or failure, polling for their raw and
runtime absence and restoration of the baseline route/config counts.

Run the existing full ExtProc suite followed by this scenario:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh \
  --full-test -r EdgionPlugins -i ExtProc
```

On an already prepared full-tier topology, with its authentication and fixture
environment exported, run `test_client -g --dynamic-scenario
ExtProcTlsPolicyRecovery`. Missing fixture identity, readiness, or counters fail
the scenario. The fast sweep reports an explicit full-tier skip; full sweeps
and resource-wide Gateway runs include it.
