# Linux Kernel and Resource Tuning

## Overview

This opt-in baseline removes common FD and listen-queue bottlenecks. It is a
starting point for a dedicated Linux gateway, not a capacity guarantee or an
automatic startup action. Test with your actual CPU quota, memory limit, protocol
mix, connection lifetime, TLS, and upstream behavior before production rollout.

**The baseline does not set `net.ipv4.tcp_tw_reuse`, including to `2`.** Existing
kernel, distribution, and operator policy remains authoritative for that setting.

## Baseline configuration

| Parameter | Type | Template value | Description |
|---|---|---|---|
| `net.core.somaxconn` | Integer | `65535` | Matches the current Edgion Pingora listen backlog; not an active-connection limit. |
| `LimitNOFILE` | Integer | `1048576` | systemd soft and hard FD allowance; does not preallocate a million sockets. |
| PAM `nofile` | Integer | `1048576` | Alternative soft/hard limit for the dedicated `edgion` account in PAM sessions. |
| `net.ipv4.tcp_tw_reuse` | Integer | Unchanged | No default override, even when the existing value is `0` or `1`. |

These are **template values**, not Linux or Edgion runtime defaults. Templates live
under `cicd/deploy/linux-tuning/` in the source checkout. Edgion does not install them itself.
Do not lower a deliberately higher existing backlog without reviewing its purpose.
Increasing a queue allows more waiting work and kernel memory use; it cannot fix a
persistently slow accept loop.

## Manual installation examples

### Preflight

Run these read-only commands on the target Linux environment:

```bash
sysctl net.core.somaxconn net.ipv4.tcp_tw_reuse
sysctl fs.nr_open fs.file-max
ulimit -Sn
ulimit -Hn
systemctl cat edgion-gateway.service
systemctl show edgion-gateway.service -p LimitNOFILE -p LimitNOFILESoft -p TasksMax
```

The service commands assume an **existing operator-provided unit** with that name.
Use your actual unit name. Record original runtime values and back up any existing
destination files outside `sysctl.d`, `limits.d`, and service drop-in directories.
Review all existing overrides: filename ordering can change the effective value.
Require `fs.nr_open >= 1048576`; otherwise choose a lower FD limit or arrange a
separately reviewed host limit change. Also budget aggregate host FD usage against
`fs.file-max`. A high per-process allowance is not host-wide resource reservation.

### Bare-metal or VM with systemd

After reviewing the templates, run from the repository root. The `test` guards
deliberately refuse to overwrite existing files; reconcile those manually.

```bash
sudo install -d -m 0755 /etc/sysctl.d
sudo test ! -e /etc/sysctl.d/99-edgion.conf && \
  sudo install -m 0644 cicd/deploy/linux-tuning/sysctl.d/99-edgion.conf /etc/sysctl.d/99-edgion.conf
sudo sysctl -p /etc/sysctl.d/99-edgion.conf

sudo install -d -m 0755 /etc/systemd/system/edgion-gateway.service.d
sudo test ! -e /etc/systemd/system/edgion-gateway.service.d/90-resource-limits.conf && \
  sudo install -m 0644 cicd/deploy/linux-tuning/systemd/edgion-gateway.service.d/90-resource-limits.conf /etc/systemd/system/edgion-gateway.service.d/90-resource-limits.conf
sudo systemctl daemon-reload
# Schedule/drain traffic before restarting the service.
sudo systemctl restart edgion-gateway.service
```

Execute each step separately and stop on failure. The sysctl command affects the
current network namespace immediately; the FD change needs a service restart.
This guide does not provide a complete service unit or prescribe `LimitNPROC`.
For thread/process creation failures, inspect `TasksMax`, cgroup `pids.max`, and
actual task counts. `LimitNPROC` is per real UID and is not a connection limit.

### PAM alternative

For a non-systemd launch through a PAM session, adjust the dedicated account in
`cicd/deploy/linux-tuning/limits.d/99-edgion.conf`, verify the login path loads `pam_limits`,
and manually install it as `/etc/security/limits.d/99-edgion.conf` after backing up
any existing file. Start a **new session**, check `ulimit -Sn` and `ulimit -Hn`,
then restart the gateway through that session. This does not configure systemd
services, already-running processes, or container-runtime limits.

### Containers and Kubernetes

Set the FD limit through the container runtime or node service configuration and
verify the actual gateway process. An unprivileged shell cannot raise its inherited hard limit.
For network sysctls, inspect the **Pod's network namespace**, not only the node:
node settings need not determine an existing Pod's effective values. Kubernetes
admits only allowed namespaced sysctls; unsafe sysctls require explicit node and
cluster policy, and `net.*` Pod sysctls are forbidden with `hostNetwork`.
Do not install a privileged tuning DaemonSet as part of this baseline. Coordinate
node policy with the cluster administrator; see the
[Kubernetes sysctl guide](https://kubernetes.io/docs/tasks/administer-cluster/sysctl-cluster/).

## Conditional tuning, not defaults

| Parameter | Type | Baseline action | When to investigate |
|---|---|---|---|
| `tcp_tw_reuse` | Integer | Preserve | Verified outbound ephemeral-port pressure after improving connection reuse. |
| `ip_local_port_range`, `ip_local_reserved_ports` | Range / list | Preserve | Port allocation failures; account for service ports and reserved ranges before widening. |
| `tcp_max_syn_backlog` | Integer | Preserve | SYN queue pressure; diagnose traffic bursts/attacks separately from the completed accept queue. |
| `netdev_max_backlog` | Integer | Preserve | NIC/softnet drops after checking CPU, RSS, IRQ distribution, and packet processing. |
| `tcp_rmem`, `tcp_wmem`, `rmem_max`, `wmem_max` | Bytes / triplets | Preserve | Measured bandwidth-delay needs; retain autotuning and budget aggregate socket memory. |
| `tcp_fin_timeout` | Seconds | Preserve | Orphaned `FIN_WAIT_2`, **not** a `TIME_WAIT` lifetime control. |
| `tcp_keepalive_time`, `tcp_keepalive_intvl`, `tcp_keepalive_probes` | Seconds / count | Preserve | Dead-peer detection requirements; only relevant to sockets using TCP keepalive, subject to per-socket overrides. |

The current [Linux TCP documentation](https://docs.kernel.org/networking/ip-sysctl.html)
defines `tcp_tw_reuse` as `0` disabled, `1` global, and `2` loopback-only, with a
documented default of `2`. Older/vendor kernels may differ. Read the deployed
kernel's documentation and effective value. Setting `1` expands reuse beyond
loopback and is not required for every proxy. Consider it only after confirming
outbound connection churn and testing real peers, timestamps, NAT, and failure
behavior; do not lower `tcp_tw_reuse_delay` blindly. Preserve the original value
for rollback. A large `TIME_WAIT` count alone is not evidence of failure, and
closed `TIME_WAIT` sockets do not consume live process FDs.

Do not copy historical recipes containing `tcp_tw_recycle`. The baseline does not
disable TCP safety mechanisms or unconditionally expand port ranges and buffers.

## Runtime and allocator coordination

These are existing process-local YAML settings, requiring a restart, not CRD fields:

```yaml
server:
  threads: 0
  work_stealing: true
  upstream_keepalive_pool_size: 128
```

`threads: 0` or omission uses available parallelism (fallback: one thread).
Check the effective CPU quota and throttling; do not blindly use host core count in
a constrained container. Work stealing defaults to `true`. The pool defaults to
`128` per worker in configuration; the current fork derives a connector pool cap
as `pool_size * threads`. This is **not a gateway-wide cap on all active and idle
upstream connections**. Multiple pools/connectors and HTTP/2 multiplexing matter.
Larger pools may reduce handshakes and port churn but retain FDs and memory.

The default allocator build uses prefixed tikv-jemalloc. Its environment setting is
`_RJEM_MALLOC_CONF`, not an assumed unprefixed `MALLOC_CONF`. For a measured staging
experiment only, set before process startup:

```bash
export _RJEM_MALLOC_CONF='background_thread:true,dirty_decay_ms:30000,muzzy_decay_ms:30000'
```

This is not installed by the baseline. Longer decay can retain more resident
memory; background purging consumes threads/CPU. Compare RSS, cgroup memory,
latency, and CPU against an unchanged control. This variable does not tune
mimalloc/system-allocator builds. Heap profiling needs a jemalloc build with
profiling support; the ordinary build does not enable that feature. Use a separate
diagnostic build, account for profiling overhead, and protect heap dumps as
sensitive data. See the [jemalloc manual](https://jemalloc.net/jemalloc.3.html).

## Capacity, verification, and alerts

Budget `downstream FDs + active upstream FDs + idle upstream FDs + other FDs +
headroom` below the process soft limit. A simple one-to-one proxy can need roughly
two socket FDs per connection pair, but that is not a universal sizing formula.
HTTP/2, streaming, plugins, and concurrent upstream work change the relationship.
One million allowed FDs does **not** mean one million safe connections.

```bash
edgion_pid=$(systemctl show edgion-gateway.service -p MainPID --value)
# Confirm a nonzero PID belonging to the gateway before proceeding.
cat /proc/"$edgion_pid"/limits
ls -1 /proc/"$edgion_pid"/fd | wc -l
ss -lnt
ss -s
ss -tan state time-wait | wc -l
nstat -az TcpExtListenOverflows TcpExtListenDrops
sysctl net.core.somaxconn net.ipv4.tcp_tw_reuse
```

Use suitable permissions and the gateway's network namespace. Minimal containers
may need a diagnostic tool container sharing that namespace. `ss`/`nstat` counters
are namespace-wide, not per-gateway metrics; compare deltas over a test interval.
The `ss` line count includes a header. For a listening TCP socket, `ss -lnt`
shows queued connections and the backlog ceiling; confirm actual listeners after
restart. Use `/proc/<pid>/limits`, not only your interactive shell's `ulimit`.

Load-test steady traffic, connection bursts/churn, slow clients/upstreams, long
streams, and rolling restarts. Record throughput, p95/p99 latency, errors, CPU
throttling, RSS/cgroup memory and OOM events, FD utilization, queue drops, port
allocation failures, and conntrack occupancy where applicable. Measure incremental
memory per connection under your workload rather than assuming a fixed byte cost.

Keep CPU **and** memory/FD/queue/port/conntrack alerts. Example FD warning/critical
thresholds of 70%/85% with sustained windows are starting points, not universal
defaults. Collect OS signals through your host/process monitoring; this guide
does not add built-in Edgion metrics. Increasing allowances is not overload
protection: keep deployment-appropriate admission/rate limits and load shedding.

## Rollback

Restore saved configuration files, or move only the newly installed Edgion files
out of their scanned configuration directories. Removing a sysctl file does not
undo a live value: explicitly restore the recorded `net.core.somaxconn` value
using `sudo sysctl -w net.core.somaxconn=VALUE` (replace `VALUE`). Recheck it now
and after the next reboot. Leave `tcp_tw_reuse` untouched unless you explicitly
changed it during a separate experiment, in which case restore its recorded value.

After restoring/removing the systemd drop-in, run `daemon-reload`, schedule a
service restart, and verify `/proc/<pid>/limits` again. PAM rollback requires a
new session and process restart. Remove experimental allocator environment
settings and restart the process. Drain traffic before reducing resource limits.

## Related documentation

- [Gateway process configuration](./configuration/gateway-config.md)
- [Monitoring](./observability/metrics.md)
- [Linux network sysctls](https://docs.kernel.org/admin-guide/sysctl/net.html)
- [systemd resource limits](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)
- [PAM limits](https://man7.org/linux/man-pages/man8/pam_limits.8.html)
- [Envoy resource limits](https://www.envoyproxy.io/docs/envoy/latest/faq/configuration/resource_limits): coordinate connection limits with FD headroom.
- [Envoy edge best practices](https://www.envoyproxy.io/docs/envoy/latest/configuration/best_practices/edge): memory/overload protection remains necessary.
- [Traefik HTTP transport](https://doc.traefik.io/traefik/reference/routing-configuration/http/load-balancing/serverstransport/) and [TCP in-flight connections](https://doc.traefik.io/traefik/reference/routing-configuration/tcp/middlewares/inflightconn/): connection reuse and explicit concurrency controls are distinct tuning tools.

These Envoy/Traefik references inform the operational approach; they are not
evidence that either project recommends `tcp_tw_reuse=1` as a modern default.
