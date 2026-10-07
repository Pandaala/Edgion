# Standalone Package and Service Verification

This directory owns the arm64 Docker lifecycle matrix for the supported
[standalone packages](../packaging/README.md). It requires actual packages built from the
current source, two revisions of each package, and a native arm64 Linux Docker
engine capable of running systemd in private cgroup namespaces.

Build both `0.1.0-1` and `0.1.0-2` package sets with the packaging scripts, using
the same current-source binaries. Then run from the repository root:

```bash
cicd/deploy/standalone/tests/run-arm64.sh \
  --packages tmp/standalone-packaging/packages \
  --report-dir tmp/standalone-packaging/reports
```

The default targets are Debian 12 and Ubuntu 24.04 (DEB), and Rocky Linux 9
(RPM). Use `--targets debian12`, `--targets ubuntu2404`, or `--targets rocky9`
for one target. Package filenames are discovered using native package metadata;
all three roles and both revisions must be available below the package directory.

The runner builds runtime images from versioned distributions, starts real
systemd as PID 1, mounts packages and tests read-only, and removes only containers
created by that invocation. Containers are privileged to give their systemd PID 1
access to its private cgroup hierarchy. The runner never mounts the engine's host
cgroup tree, exposes host ports, or applies host sysctls. Runtime images remain
cached for repeated tests.

`lifecycle.sh prepare` checks install metadata and ELF dependencies, exclusive
file ownership and native conffile/noreplace metadata, disabled/stopped first
installs, daemon identities and protected
paths, complete CRD loading, explicit mTLS initialization, both no-token gates,
CLI CRUD, and a real HTTP request through port 80 to the in-container backend.
The EndpointSlice fixture is rendered privately with the container's own
non-loopback IPv4 interface address. The backend listens on `0.0.0.0:18080`
inside the isolated Docker network namespace with no published host ports.
This exercises ordinary production backend-address policy without enabling
integration mode or a loopback-upstream override.
The auth checks initially start the Gateway with no resource listeners. After
applying the fixture, a silent authenticated production-cache barrier confirms
its GatewayClass and HTTP port 80 Gateway have arrived, then explicitly restarts
the Gateway once before asserting readiness and the complete proxy response.
This follows the documented startup-only listener-port contract: applying a new
port to an already-running Gateway requires restart. The restart is unconditional
bootstrap sequencing, not a retry after failed traffic; later proxy failures
still fail normally.
It rejects pre-install configuration/directory symlinks, account UID aliases,
and numeric GID aliases with supplementary viewer membership
using actual packaged scripts without changing collision targets. It checks
independent Controller restart, an acknowledged in-flight request
during SIGTERM, configuration/key/port failures and recovery, upgrades, package
rollback, preservation hashes, and stopped/disabled/masked states. A short test
drain budget is configured explicitly; vendor stop budgets remain unchanged.

Interrupted-operation checks invoke actual installed DEB maintainer scripts or
scripts extracted from the actual RPM across a stop/resume boundary. They verify
persisted restart intent and successful recovery, without claiming to simulate a
crashed package database. DEB verifies failed-upgrade deferral, abort-remove
restoration, a native `policy-rc.d` denial, later disabled transactions, and
explicit `EDGION_PACKAGE_RECOVER=1` configure recovery.
Both formats also check installed recovery helpers, mask preservation, required
explicit opt-in and `--cancel` of deferred restart intent. Preservation hashes
include real generated resource status files. Both revisions ship identical
defaults; native protection flags are checked separately and this matrix does
not claim to test merging changed vendor configuration defaults.
The harness then restarts the Docker container
and runs `lifecycle.sh finish`, verifying persisted enablement, readiness, proxy
traffic, removal, native configuration purge semantics, retained business data,
reinstallation, unchanged namespace sysctls and a separate explicit data/log
purge after backup. RPM has no native purge;
modified configuration is checked through `.rpmsave`.

Reports contain stage results, target identity, package revision pairing, and
unchanged sysctl values. Tokens, private keys, environment contents, CLI output,
and secret-bearing process configs stay inside the disposable container. Failures
stop the matrix; only the latest native package-manager output is additionally
copied to `<target>-package-error.log` before cleanup. Journals, CLI output and
the private test-state directory are never exported.

After a proxy retry deadline, `proxy_diagnostics.py` emits one allowlisted
snapshot into the lifecycle report. A hard 15-second process deadline includes
file I/O; individual probes use at most two seconds or the remaining budget.
The snapshot includes HTTP statuses, fixed service-state fields, a boolean
confirming the private backend address belongs to this namespace, and at most
five canonical access-log statuses/error codes. File reads reject symlinks and
FIFOs; the access-log tail is bounded to 64 KiB. No response body, headers,
addresses, configuration, environment or raw exception text is exported.
HTTP diagnostics disable environment proxies and redirects. Diagnostics never
convert a failed body assertion into success; the original timeout still fails.

Run the focused diagnostics checks inside a runtime image (Python 3.9 supported):

```bash
docker run --rm --platform linux/arm64 \
  --mount "type=bind,src=$PWD/cicd/deploy/standalone/tests,dst=/tests,readonly" \
  edgion-package-test:rocky9-arm64 \
  python3 -m unittest discover -s /tests -p 'test_*.py'
```

These checks exercise field redaction, numeric/type bounds, partial/oversized
log records, no-follow/nonblocking file handling, real local HTTP proxy/redirect
denial, probe/global deadlines and propagation of the original test failure.
Fixture-barrier checks cover the real list envelope, precise resource identity,
strict listener port types, bounded authenticated reads, silent errors and the
required barrier/restart/readiness/body-assertion sequence.
The Rocky runtime installs `diffutils` for the sysctl `cmp` check. Required
comparison tools must be present before interpreting preservation assertions.

## Validation Scope and Limits

The complete native ARM64 lifecycle matrix passed on Debian 12, Ubuntu 24.04,
and Rocky Linux 9. The 17 focused Python checks also passed. These results apply
to the tested artifacts; rebuilds must rerun the matrix.

Container restart verifies service enablement and startup, not a host reboot.
Docker shares its engine kernel; bootloaders, SELinux enforcement and physical
host capacity are outside this validation. Revisions of identical binaries prove
package upgrade/rollback behavior, not cross-product-version compatibility.
amd64 execution is not validated by this harness. Both revisions use identical
vendor defaults, so changed-default merging is not exercised. Hook interruption
checks do not simulate a crashed package database. Local verification does not
establish a GitHub Actions run or published release.

Keep generated lifecycle logs and JSON reports in the selected ignored report
directory; retain package checksums and ELF metadata alongside package artifacts.
