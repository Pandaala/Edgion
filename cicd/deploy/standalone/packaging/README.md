# Standalone DEB and RPM Packages

This directory owns independent `edgion-controller`, `edgion-gateway`, and
`edgion-cli` packages. Gateway does not require a local Controller; CLI has no
service. Supported targets are Debian 12 and Ubuntu 24.04 (DEB), and Rocky
Linux 9 (RPM), with systemd for both daemons. See the
[test guide](../tests/README.md) for ARM64 verification and its limits.

## Build

From the repository root, build current-source binaries in a native Rocky 9
container using Rust 1.96.1, Cargo.lock, the `production` profile, and ordinary
BoringSSL defaults:

```bash
cicd/deploy/standalone/packaging/scripts/build-binaries.sh \
  --arch arm64 --output tmp/standalone-packaging/binaries
for revision in 1 2; do
  cicd/deploy/standalone/packaging/scripts/build-packages.sh \
    --arch arm64 --version 0.1.0 --release "$revision" \
    --binaries tmp/standalone-packaging/binaries \
    --output tmp/standalone-packaging/packages
done
```

Docker and Python 3.9+ are required. `--image IMAGE` reuses a native builder.
Named volumes `edgion-packaging-{cargo,git,target}-arm64` cache downloads/builds;
the checkout is mounted read-only. The builder defaults to one Cargo job to
limit compilation concurrency, but production compilation still needs
substantial memory. The validated build environment used 12 GiB Docker engine
memory and 8 GiB swap; this is a build reference, not a runtime capacity
requirement. Package creation alone does not establish runtime compatibility;
execute the [lifecycle matrix](../tests/README.md).
`--arch amd64` is available without an
amd64 runtime-validation claim. No FIPS or alternate Gateway TLS build is
provided. Package output belongs in ignored directories such as `tmp/`.

The package entry accepts `--format deb|rpm|all` and `--stage-only` for
inspection without Docker. Input binaries must be named `edgion-controller`,
`edgion-gateway`, and `edgion-cli`. It validates ELF64 architecture, dynamic
libraries, and glibc requirements against the 2.34 baseline, and rejects
unmapped runtime libraries. Native dependencies are derived per binary. Target
execution remains necessary even after ELF inspection.

nFPM 2.47.0 uses a pinned multi-platform image digest. Names follow
`edgion-controller_0.1.0-1_arm64.deb` and
`edgion-controller-0.1.0-1.aarch64.rpm`, with equivalent names for other roles.
Output includes ELF metadata and SHA-256 checksums. Revisions 1 and 2 use the
same binaries: package rollback does not establish arbitrary application-version
compatibility.

## Install local artifacts

On Debian 12 or Ubuntu 24.04 arm64, install the roles needed on this host:

```bash
sudo apt install ./edgion-controller_0.1.0-1_arm64.deb \
  ./edgion-gateway_0.1.0-1_arm64.deb ./edgion-cli_0.1.0-1_arm64.deb
```

On Rocky Linux 9 aarch64, use the equivalent RPM artifacts:

```bash
sudo dnf install ./edgion-controller-0.1.0-1.aarch64.rpm \
  ./edgion-gateway-0.1.0-1.aarch64.rpm ./edgion-cli-0.1.0-1.aarch64.rpm
```

Run these commands in the package output directory and omit unneeded roles.
`apt` and `dnf` resolve the declared native dependencies through configured
repositories. A Gateway-only host can install Gateway and CLI independently of
Controller. First installation leaves services stopped and disabled; initialize
credentials and resources before explicitly enabling or starting them.

## Installed ownership

| Asset | Path | Policy |
|-------|------|--------|
| Binaries | `/usr/bin/edgion-*` | Package-owned root executables |
| Process YAML | `/etc/edgion/<role>/edgion-<role>.yaml` | root:service 0640; conffile/noreplace |
| Environment | `/etc/edgion/<role>/environment` | root:service 0640; no default token |
| Certificates | `/etc/edgion/<role>/certs/` | root:service 0750 directory; operator-owned files |
| Work directory | `/var/lib/edgion/<role>/` | root:service 1770; retained |
| Protected config | Workdir `config/` | root:root 0755 |
| Runtime and work logs | Workdir `runtime/`, `logs/` | service:service 0750; retained |
| FileSystem resources | `/var/lib/edgion/controller/resources/` | Controller-owned 0750; retained |
| Actual logs | `/var/log/edgion/<role>/` | service:service 0750; retained |
| Vendor units | `/usr/lib/systemd/system/edgion-<role>.service` | Controller/Gateway only |
| CRDs | `/usr/share/edgion-controller/crd/` | Controller package owns complete `config/crd` tree |
| Recovery helper | `/usr/libexec/edgion-<role>/package-recover` | Package-owned root executable |
| Docs, license, tuning | `/usr/share/doc/edgion-<role>/` | Inert examples; tuning never applied |

Controller has an exact `config/crd` symlink to the shared schema directory,
including Edgion CRDs and both vendored Gateway API profiles. Its sticky
workdir permits the binary write check but protects root-owned config entries.
Hooks reject conflicting accounts, duplicate UIDs, supplementary groups,
unsafe ancestors, symlinks, owners, or modes. They never recursively change
ownership. Debian archives use numeric GID 0 for conffiles; postinst assigns
the service group only to the two exact validated root-owned regular files.

## Initialize explicitly

First install creates accounts, protected defaults, directories, and units,
leaving services disabled and stopped. In each protected environment file,
set an operator-managed token using systemd syntax: `EDGION_ADMIN_TOKEN` for
Controller and a separate `EDGION_GATEWAY_ADMIN_TOKEN` for Gateway. Preserve
root:service ownership and 0640 mode. Token rotation requires service restart.
No hook generates a token, downloads assets, or puts credentials in arguments.

Generate same-host mTLS certificates once in private root-owned staging:

```bash
umask 077
cert_stage=$(mktemp -d)
edgion-cli gen-certs --output file --output-dir "$cert_stage" \
  --server-san 127.0.0.1 --server-san localhost
```

Install `primary/{ca.crt,server.crt,server.key}` only into Controller's certs
directory, and `primary/{ca.crt,client.crt,client.key}` only into Gateway's.
Use `install -o root -g edgion-controller -m 0640` and the equivalent Gateway
group. Verify all destinations are absent before copying; abort if any existing
credential would be overwritten. Back up the generated certificate bundle in
operator-owned storage. The CLI does not export its CA signing key; when using
an external issuer, its signing key must remain outside both service directories.
Gateway must not read Controller's server key or an issuer signing key.

Install resource YAML into Controller's writable resources directory, including
write access for adjacent `.status` files. Start Controller, verify
`http://127.0.0.1:12100/ready` and authenticated CLI access, then start Gateway
and verify `http://127.0.0.1:12000/ready` plus actual proxy traffic. Enable boot
startup explicitly after initialization succeeds.

For split hosts, add reachable server SANs, change Controller's gRPC bind and
Gateway's `server_addrs`, and copy only Gateway's certificate set to its host.
Admin, probe, and metrics defaults bind loopback; exposure requires an explicit
configuration and network-policy decision. See the [unit contract](../systemd/README.md).

## Upgrade and recover

DEB captures active state in old `prerm upgrade`, stops gracefully, and resumes
only from terminal `postinst configure` or `postinst abort-upgrade`. Retry
`failed-upgrade` hooks retain the marker and leave the service stopped until
replacement/unwind completes. Preinstall tools are declared with Pre-Depends.
RPM captures active state in new `%pre`; old upgrade `%preun`/`%postun` remain
inert; new `%posttrans` resumes after old uninstall scriptlets.

Root-only restart markers live at `/var/lib/edgion/.packaging/<role>.restart`.
Native Debian `policy-rc.d` and service masks can defer restart. The marker then
becomes `deferred`: later ordinary package operations do not start a service
that the operator deliberately left stopped. To explicitly recover after fixing
policy/configuration, use:

```bash
sudo env EDGION_PACKAGE_RECOVER=1 /usr/libexec/edgion-controller/package-recover
sudo env EDGION_PACKAGE_RECOVER=1 /usr/libexec/edgion-gateway/package-recover
```

Recovery still respects native policy and masks; it never unmasks a unit.
Use the same helper with `--cancel` to cancel validated pending restart/removal
intent without starting a service. A failed restart retains active intent for
retry; a subsequent capture of an actually active daemon refreshes that intent.

Removal first records active and enablement states. An interrupted removal
restores prior enablement and activity through native DEB abort-remove or
successful reinstall recovery. Final removal clears those markers, so a later
fresh reinstall remains stopped and disabled. Normal upgrades preserve
enabled/disabled/masked states and operator overrides.

Package transactions are not atomic across Controller and Gateway. Upgrade
Controller first and verify readiness, then upgrade Gateway and verify traffic.
Do not infer hot upgrade or arbitrary product-version rollback from package
revision testing.

## Remove and purge

Removal stops/disables the removed daemon and retains accounts, credentials,
data, logs, and operator unit overrides. Mask overrides remain. DEB
`dpkg --purge` removes only native package-owned conffiles; certificates are
not conffiles. RPM retains modified configs as `.rpmsave` through native
`%config(noreplace)` semantics. RPM has no purge operation: explicitly remove
only the process YAML, environment file, and their `.rpmsave`/`.rpmnew`
variants after removal when desired. Do not delete the entire role config
directory, which also contains credentials.

Data/log deletion is a separate operator action on named paths after backup.
Package hooks never apply sysctls, PAM limits, container-runtime limits, or
unrelated service policy. [Optional shared tuning](../../linux-tuning/README.md)
ships only as inert documentation.
