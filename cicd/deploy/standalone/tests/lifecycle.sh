#!/usr/bin/env bash
# This script runs as root INSIDE a disposable, native arm64 systemd container.
set -euo pipefail
format=${1:?deb or rpm required}
phase=${2:?prepare or finish required}
state=/var/lib/package-test
mkdir -p "$state"
chmod 700 "$state"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }
proxy_timeout_diagnostics() {
    # Hard process deadline also covers file I/O; never export raw error text.
    if ! timeout --signal=KILL 15s python3 /tests/proxy_diagnostics.py 2>/dev/null; then
        printf '%s\n' '{"proxy_timeout_diagnostics":"unavailable"}'
    fi
}
wait_until() {
    local attempt
    for ((attempt=0; attempt<90; attempt++)); do if "$@" >/dev/null 2>&1; then return 0; fi; sleep 1; done
    if [[ "${1:-}" == proxy ]]; then proxy_timeout_diagnostics || true; fi
    fail "condition timed out: $1"
}
ready() { curl -fsS --max-time 2 "http://127.0.0.1:$1/ready" >/dev/null; }
fixture_ready() { python3 /tests/gateway_fixture_ready.py "$state"; }
proxy() { [[ $(curl -fsS --max-time 5 -H 'Host: package.test' http://127.0.0.1/) == edgion-package-backend ]]; }
cli() {
    EDGION_TOKEN=$(cat "$state/controller-token") edgion-cli --server http://127.0.0.1:12101 "$@"
}
quiet_cli() { cli "$@" >"$state/cli-output" 2>&1 || fail "CLI operation failed: $1"; }
package_path() {
    local role=$1 revision=$2 path metadata
    while IFS= read -r path; do
        if [[ "$format" == deb ]]; then
            metadata=$(dpkg-deb -f "$path" Package Version Architecture)
            if [[ "$metadata" == $'Package: edgion-'"$role"$'\nVersion: 0.1.0-'"$revision"$'\nArchitecture: arm64' ]]; then printf '%s\n' "$path"; return; fi
        else
            metadata=$(rpm -qp --qf '%{NAME} %{VERSION}-%{RELEASE} %{ARCH}' "$path" 2>/dev/null)
            if [[ "$metadata" == "edgion-$role 0.1.0-$revision aarch64" ]]; then printf '%s\n' "$path"; return; fi
        fi
    done < <(find /packages -type f -name "*.$format" | sort)
    fail "missing $format edgion-$role arm64 revision $revision"
}
install_set() {
    local revision=$1 role path
    for role in controller gateway cli; do
        path=$(package_path "$role" "$revision")
        if [[ "$format" == deb ]]; then
            DEBIAN_FRONTEND=noninteractive dpkg --force-confold -i "$path" >"$state/package-output" 2>&1 || fail "DEB transaction failed ($role revision $revision)"
        else
            rpm -U --oldpackage --replacepkgs "$path" >"$state/package-output" 2>&1 || fail "RPM transaction failed ($role revision $revision)"
        fi
    done
}
snapshot() {
    python3 - "$state" "$1" <<'PY'
import hashlib, json, pathlib, sys
base, mode = pathlib.Path(sys.argv[1]), sys.argv[2]
paths = sorted(pathlib.Path('/etc/edgion').rglob('*'))
paths += sorted(pathlib.Path('/etc/systemd/system').glob('edgion-*.service.d/*'))
paths += sorted(pathlib.Path('/var/lib/edgion/controller/resources').rglob('*.yaml'))
paths += sorted(pathlib.Path('/var/lib/edgion/controller/resources').rglob('*.status'))
paths += [pathlib.Path('/var/lib/edgion/controller/resources/operator-state.status'),
          pathlib.Path('/var/lib/edgion/controller/resources/operator-state'),
          pathlib.Path('/var/lib/edgion/controller/resources/operator-data')]
current = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths if p.is_file()}
stored = base / 'preserved.json'
if mode == 'save':
    stored.write_text(json.dumps(current))
elif mode == 'removed':
    for name, digest in json.loads(stored.read_text()).items():
        p = pathlib.Path(name)
        if not p.is_file():
            p = pathlib.Path(name + '.rpmsave')
        assert p.is_file() and hashlib.sha256(p.read_bytes()).hexdigest() == digest, 'removed package lost operator file: ' + name
else:
    assert json.loads(stored.read_text()) == current, 'operator configuration/data hashes changed'
PY
}
sysctl_snapshot() {
    python3 - <<'PY'
import json, pathlib
paths = ['net/core/somaxconn', 'net/ipv4/tcp_tw_reuse', 'net/ipv4/tcp_fin_timeout',
         'net/ipv4/ip_local_port_range', 'fs/file-max']
print(json.dumps({p: pathlib.Path('/proc/sys', p).read_text().strip() for p in paths}, sort_keys=True))
PY
}
check_invariants() {
    local role group other
    for role in controller gateway; do
        group="edgion-$role"
        [[ $(stat -c '%U:%G:%a' "/var/lib/edgion/$role") == "root:$group:1770" ]] || fail "unsafe $role workdir"
        [[ $(stat -c '%U' "/var/lib/edgion/$role/config") == root ]] || fail "unprotected $role config"
        [[ $(systemctl show "$group.service" -p User --value) == "$group" ]] || fail "incorrect service identity"
        [[ $(systemctl show "$group.service" -p KillSignal --value) == 15 ]] || fail "incorrect stop signal"
        [[ $(systemctl show "$group.service" -p Restart --value) == on-failure ]] || fail "incorrect restart policy"
        [[ $(systemctl show "$group.service" -p LimitNOFILE --value) == 65536 ]] || fail "incorrect file limit"
        if [[ "$role" == controller ]]; then other=gateway; else other=controller; fi
        ! runuser -u "$group" -- test -r "/etc/edgion/$other/environment" || fail "cross-role environment readable"
        ! runuser -u "$group" -- mv "/var/lib/edgion/$role/config" "/var/lib/edgion/$role/config-moved" 2>/dev/null || fail "daemon can replace protected config"
        runuser -u "$group" -- touch "/var/lib/edgion/$role/runtime/package-write-check"
    done
    [[ $(readlink /var/lib/edgion/controller/config/crd) == /usr/share/edgion-controller/crd ]] || fail "CRD lookup link missing"
    [[ -d /usr/share/edgion-controller/crd/edgion-crd && -d /usr/share/edgion-controller/crd/gateway-api ]] || fail "schema tree missing"
    ! runuser -u edgion-gateway -- test -r /etc/edgion/controller/certs/server.key || fail "Gateway can read Controller key"
    ! runuser -u edgion-controller -- test -r /etc/edgion/gateway/certs/client.key || fail "Controller can read Gateway key"
    [[ $(id -u edgion-controller) != "$(id -u edgion-gateway)" ]] || fail "daemon identities share UID"
    [[ -z $(find /etc/edgion/gateway -name ca.key) ]] || fail "Gateway has CA signing key"
    [[ $(systemctl show edgion-controller -p TimeoutStopUSec --value) == 45s ]] || fail "Controller stop budget"
    [[ $(systemctl show edgion-gateway -p TimeoutStopUSec --value) == '1min 30s' ]] || fail "Gateway stop budget"
    [[ $(systemctl show edgion-gateway -p CapabilityBoundingSet --value) == cap_net_bind_service ]] || fail "Gateway capability set"
    [[ $(systemctl show edgion-gateway -p AmbientCapabilities --value) == cap_net_bind_service ]] || fail "Gateway ambient capability"
    [[ " $(systemctl show edgion-gateway -p Requires --value) " != *' edgion-controller.service '* ]] || fail "hard local Controller dependency"
    pass "independent identities, protected paths, schemas, stop budgets, capabilities"
}
initialize() {
    local role kind
    install -d -m 700 "$state/cert-generation"
    edgion-cli gen-certs --output file --output-dir "$state/cert-generation" --server-san 127.0.0.1 --server-san localhost >"$state/cert-output" 2>&1
    for role in controller gateway; do
        install -d -o root -g "edgion-$role" -m 750 "/etc/edgion/$role/certs"
        if [[ "$role" == controller ]]; then kind=server; else kind=client; fi
        for file in ca.crt "$kind.crt" "$kind.key"; do
            install -o root -g "edgion-$role" -m 640 "$state/cert-generation/primary/$file" "/etc/edgion/$role/certs/$file"
        done
    done
    # Private test material remains in-container and is never copied to reports.
    python3 - "$state" <<'PY'
import pathlib, secrets, sys
base = pathlib.Path(sys.argv[1])
for role, variable in [('controller', 'EDGION_ADMIN_TOKEN'), ('gateway', 'EDGION_GATEWAY_ADMIN_TOKEN')]:
    token = secrets.token_urlsafe(48)
    (base / (role + '-token')).write_text(token)
    pathlib.Path('/etc/edgion', role, 'environment').write_text(variable + '=' + token + '\n')
PY
    chmod 640 /etc/edgion/{controller,gateway}/environment
    for role in controller gateway; do
        printf '\n# Operator modification retained across package transactions.\n' >>"/etc/edgion/$role/edgion-$role.yaml"
    done
    # Shorten drain in this fixture while preserving the package's stop timeout.
    python3 - <<'PY'
from pathlib import Path
p = Path('/etc/edgion/gateway/edgion-gateway.yaml')
s = p.read_text()
s = s.replace('grace_period_seconds: 30', 'grace_period_seconds: 5')
s = s.replace('graceful_shutdown_timeout_seconds: 10', 'graceful_shutdown_timeout_seconds: 5')
p.write_text(s)
PY
    cp -p /etc/edgion/gateway/edgion-gateway.yaml "$state/gateway-config-backup"
    install -d -m 755 /etc/systemd/system/edgion-controller.service.d /etc/systemd/system/edgion-gateway.service.d
    printf '[Service]\nEnvironment=EDGION_PACKAGE_TEST_OVERRIDE=preserved\n' >/etc/systemd/system/edgion-controller.service.d/operator.conf
    printf '[Service]\nEnvironment=EDGION_PACKAGE_TEST_OVERRIDE=preserved\n' >/etc/systemd/system/edgion-gateway.service.d/operator.conf
    cat >/etc/systemd/system/package-backend.service <<'UNIT'
[Unit]
Description=Disposable package verification backend
[Service]
ExecStart=/usr/bin/python3 /tests/backend.py
Restart=on-failure
[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable --now package-backend.service
    python3 - "$state" <<'PY'
import ipaddress, json, pathlib, subprocess, sys
interfaces = json.loads(subprocess.check_output(['ip', '-j', '-4', 'addr', 'show', 'scope', 'global'], text=True))
addresses = [ipaddress.IPv4Address(address['local']) for interface in interfaces if interface['ifname'] != 'lo'
             for address in interface.get('addr_info', []) if address.get('family') == 'inet' and address.get('scope') == 'global']
addresses = [address for address in addresses if not (address.is_loopback or address.is_unspecified or address.is_link_local or address.is_multicast)]
assert addresses, 'container requires a non-loopback IPv4 interface'
template = pathlib.Path('/tests/fixtures/resources.yaml').read_text()
assert template.count('__BACKEND_IPV4__') == 1, 'backend address placeholder contract changed'
pathlib.Path(sys.argv[1], 'resources.yaml').write_text(template.replace('__BACKEND_IPV4__', str(addresses[0])))
pathlib.Path(sys.argv[1], 'backend-ipv4').write_text(str(addresses[0]) + '\n')
PY
}
failure_recovery() {
    local config=/etc/edgion/controller/edgion-controller.yaml
    systemctl stop edgion-controller
    cp -p "$config" "$state/controller-config-backup"
    printf 'server: [invalid\n' >"$config"
    systemctl start edgion-controller || true
    sleep 2
    ! ready 12100 || fail "invalid config became ready"
    systemctl stop edgion-controller
    cp -p "$state/controller-config-backup" "$config"
    systemctl reset-failed edgion-controller
    systemctl start edgion-controller
    wait_until ready 12100
    wait_until proxy
    pass "invalid process configuration fails and recovers"
    systemctl stop edgion-controller
    mv /etc/edgion/controller/certs/server.key "$state/server.key"
    systemctl start edgion-controller || true
    sleep 2
    ! ready 12100 || fail "missing certificate became ready"
    systemctl stop edgion-controller
    mv "$state/server.key" /etc/edgion/controller/certs/server.key
    systemctl reset-failed edgion-controller
    systemctl start edgion-controller
    wait_until ready 12100
    wait_until proxy
    pass "missing mTLS key fails and recovers"
    systemctl stop edgion-controller
    systemd-run --unit=package-port-conflict /usr/bin/python3 -m http.server 12151 --bind 127.0.0.1 >/dev/null
    wait_until curl -fsS --max-time 1 http://127.0.0.1:12151/
    systemctl start edgion-controller || true
    sleep 2
    ! ready 12100 || fail "gRPC port conflict became ready"
    systemctl stop edgion-controller package-port-conflict
    systemctl reset-failed edgion-controller
    systemctl start edgion-controller
    wait_until ready 12100
    wait_until proxy
    pass "gRPC port conflict fails and recovers"
}
interrupted_hook_recovery() {
    local pre post
    if [[ "$format" == deb ]]; then
        sh /var/lib/dpkg/info/edgion-controller.prerm upgrade 0.1.0-2
        [[ -f /var/lib/edgion/.packaging/controller.restart ]] || fail "upgrade did not persist restart intent"
        ! systemctl is-active --quiet edgion-controller || fail "upgrade hook did not stop old daemon"
        sh /var/lib/dpkg/info/edgion-controller.postrm failed-upgrade 0.1.0-2
        [[ -f /var/lib/edgion/.packaging/controller.restart ]] || fail "failed-upgrade lost restart intent"
        ! systemctl is-active --quiet edgion-controller || fail "failed-upgrade resumed before terminal recovery"
        sh /var/lib/dpkg/info/edgion-controller.postinst abort-upgrade 0.1.0-2
    else
        pre="$state/controller-preinstall"
        post="$state/controller-posttrans"
        rpm -qp --qf '%{PREIN}' "$(package_path controller 2)" >"$pre"
        rpm -qp --qf '%{POSTTRANS}' "$(package_path controller 2)" >"$post"
        sh "$pre" 2
        [[ -f /var/lib/edgion/.packaging/controller.restart ]] || fail "RPM upgrade did not persist restart intent"
        ! systemctl is-active --quiet edgion-controller || fail "RPM preinstall did not stop old daemon"
        # Resume the packaged post-transaction hook after an interruption between hooks.
        sh "$post" 2
    fi
    wait_until ready 12100
    wait_until proxy
    [[ ! -e /var/lib/edgion/.packaging/controller.restart ]] || fail "recovery left stale restart intent"
    pass "actual packaged hook interruption/recovery retains and consumes restart intent"
    if [[ "$format" == deb ]]; then
        sh /var/lib/dpkg/info/edgion-controller.prerm remove
        ! systemctl is-active --quiet edgion-controller || fail "remove hook did not stop daemon"
        sh /var/lib/dpkg/info/edgion-controller.postinst abort-remove
        wait_until ready 12100
        wait_until proxy
        [[ $(systemctl is-enabled edgion-controller) == enabled ]] || fail "abort-remove lost prior enablement"
        pass "actual packaged abort-remove restores active and enabled service"
    fi
}
adversarial_preinstall() {
    local script argument alias_group config=/etc/edgion/controller/edgion-controller.yaml
    if [[ "$format" == deb ]]; then
        mkdir -p "$state/deb-control"
        dpkg-deb -e "$(package_path controller 1)" "$state/deb-control"
        script="$state/deb-control/preinst"
        argument=install
    else
        script="$state/controller-preinstall"
        rpm -qp --qf '%{PREIN}' "$(package_path controller 1)" >"$script"
        argument=1
    fi
    sh "$script" "$argument" >"$state/adversarial-output" 2>&1 || fail "clean preinstall validation failed"
    printf 'untouched-collision-target\n' >"$state/collision-target"
    ln -s "$state/collision-target" "$config"
    if sh "$script" "$argument" >"$state/adversarial-output" 2>&1; then fail "preinstall accepted symlinked process config"; fi
    [[ $(cat "$state/collision-target") == untouched-collision-target ]] || fail "preinstall changed symlink target"
    [[ -L "$config" ]] || fail "preinstall replaced operator symlink"
    rm "$config"
    mv /var/lib/edgion/controller/config "$state/protected-config"
    ln -s "$state/protected-config" /var/lib/edgion/controller/config
    if sh "$script" "$argument" >"$state/adversarial-output" 2>&1; then fail "preinstall accepted symlinked protected directory"; fi
    [[ -L /var/lib/edgion/controller/config ]] || fail "preinstall changed unsafe directory"
    rm /var/lib/edgion/controller/config
    mv "$state/protected-config" /var/lib/edgion/controller/config
    useradd --non-unique --uid "$(id -u edgion-controller)" --gid 65534 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin package-uid-alias
    if sh "$script" "$argument" >"$state/adversarial-output" 2>&1; then fail "preinstall accepted another account sharing daemon UID"; fi
    userdel package-uid-alias
    groupadd --non-unique --gid "$(id -g edgion-controller)" package-gid-alias
    useradd --system --gid 65534 --groups package-gid-alias --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin package-gid-viewer
    alias_group=$(getent group package-gid-alias)
    [[ "$alias_group" == *:package-gid-viewer ]] || fail "GID collision fixture lacks supplementary viewer"
    if sh "$script" "$argument" >"$state/adversarial-output" 2>&1; then fail "preinstall accepted aliased service GID with supplementary viewer"; fi
    [[ $(getent group package-gid-alias) == "$alias_group" ]] || fail "preinstall altered conflicting GID alias group"
    id package-gid-viewer >/dev/null || fail "preinstall removed conflicting viewer"
    userdel package-gid-viewer
    groupdel --force package-gid-alias
    getent group edgion-controller >/dev/null || fail "fixture cleanup removed canonical service group"
    pass "actual preinstall rejects symlink/config collisions and aliased daemon UID/GID without clobbering targets"
}
native_policy_recovery() {
    [[ "$format" == deb ]] || return 0
    printf '#!/bin/sh\nexit 101\n' >/usr/sbin/policy-rc.d
    chmod 755 /usr/sbin/policy-rc.d
    install_set 2
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "native deny-start policy ignored"
        [[ -f "/var/lib/edgion/.packaging/$role.restart" ]] || fail "policy denial lost restart intent"
        [[ $(cat "/var/lib/edgion/.packaging/$role.restart") == deferred ]] || fail "policy denial did not defer restart intent"
    done
    rm /usr/sbin/policy-rc.d
    systemctl disable edgion-controller edgion-gateway
    install_set 1
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "later transaction automatically resumed deferred service"
        [[ $(systemctl is-enabled "edgion-$role" || true) == disabled ]] || fail "deferred transaction changed operator disablement"
        [[ $(cat "/var/lib/edgion/.packaging/$role.restart") == deferred ]] || fail "later transaction discarded deferred restart intent"
        EDGION_PACKAGE_RECOVER=1 sh "/var/lib/dpkg/info/edgion-$role.postinst" configure 0.1.0-2
    done
    wait_until ready 12100
    wait_until proxy
    systemctl enable edgion-controller edgion-gateway
    pass "DEB native deny-start policy defers across later disabled transactions until explicit configure recovery"
}
deferred_mask_recovery() {
    local role
    # Masking an active unit retains its process; upgrade must stop but defer resume.
    systemctl mask edgion-controller edgion-gateway
    install_set 2
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "masked upgrade restarted daemon"
        [[ $(systemctl is-enabled "edgion-$role" || true) == masked ]] || fail "upgrade lost active-unit mask"
        [[ $(cat "/var/lib/edgion/.packaging/$role.restart") == deferred ]] || fail "masked restart was not deferred"
    done
    EDGION_PACKAGE_RECOVER=1 /usr/libexec/edgion-controller/package-recover >"$state/helper-output" 2>&1
    ! systemctl is-active --quiet edgion-controller || fail "explicit helper bypassed active mask"
    [[ $(systemctl is-enabled edgion-controller || true) == masked ]] || fail "explicit helper removed operator mask"
    systemctl unmask edgion-controller edgion-gateway
    systemctl disable edgion-controller edgion-gateway
    install_set 1
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "unmasked later transaction resumed deferred daemon"
        [[ $(systemctl is-enabled "edgion-$role" || true) == disabled ]] || fail "unmasked transaction changed disablement"
        if /usr/libexec/"edgion-$role"/package-recover >"$state/helper-output" 2>&1; then fail "recovery helper accepted missing explicit opt-in"; fi
        if [[ "$role" == controller ]]; then
            EDGION_PACKAGE_RECOVER=1 /usr/libexec/edgion-controller/package-recover >"$state/helper-output" 2>&1
        else
            /usr/libexec/edgion-gateway/package-recover --cancel >"$state/helper-output" 2>&1
            [[ ! -e /var/lib/edgion/.packaging/gateway.restart ]] || fail "cancel helper retained restart intent"
            ! systemctl is-active --quiet edgion-gateway || fail "cancel helper started daemon"
            systemctl start edgion-gateway
        fi
    done
    wait_until ready 12100
    wait_until proxy
    systemctl enable edgion-controller edgion-gateway
    pass "active masked upgrades defer across later disabled transactions; installed recovery/cancel helpers respect masks and opt-in"
}
[[ $(uname -m) == aarch64 ]] || fail "native arm64 target required"
[[ $(cat /proc/1/comm) == systemd ]] || fail "real systemd PID 1 required"
wait_until systemctl show-environment
if [[ "$phase" == prepare ]]; then
    sysctl_snapshot >"$state/sysctl-before.json"
    adversarial_preinstall
    install_set 1
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "first install started $role"
        [[ $(systemctl is-enabled "edgion-$role" || true) == disabled ]] || fail "first install enabled $role"
    done
    for role in controller gateway cli; do
        readelf -h "/usr/bin/edgion-$role" | grep -q AArch64 || fail "wrong ELF architecture"
        ldd "/usr/bin/edgion-$role" >"$state/$role-elf-dependencies" || fail "ELF dependency inspection failed"
        ! grep -q 'not found' "$state/$role-elf-dependencies" || fail "missing runtime ELF dependency"
        "/usr/bin/edgion-$role" --version >/dev/null
    done
    python3 - "$format" <<'PY'
import pathlib, subprocess, sys
owners = {}
for role in ['controller', 'gateway', 'cli']:
    package = 'edgion-' + role
    cmd = ['dpkg', '-L', package] if sys.argv[1] == 'deb' else ['rpm', '-ql', package]
    for name in subprocess.check_output(cmd, text=True).splitlines():
        p = pathlib.Path(name)
        if p.is_file() or p.is_symlink():
            assert name not in owners, 'file has multiple package owners: ' + name
            owners[name] = package
for role in ['controller', 'gateway']:
    package = 'edgion-' + role
    required = {'/etc/edgion/' + role + '/edgion-' + role + '.yaml',
                '/etc/edgion/' + role + '/environment'}
    if sys.argv[1] == 'deb':
        output = subprocess.check_output(['dpkg-query', '-W', '-f', '${Conffiles}', package], text=True)
        actual = {line.split()[0] for line in output.splitlines() if line.strip()}
        assert required <= actual, 'missing DEB conffile protection'
    else:
        output = subprocess.check_output(['rpm', '-q', '--qf', '[%{FILENAMES}\t%{FILEFLAGS}\n]', package], text=True)
        actual = {name for name, flags in (line.split('\t') for line in output.splitlines()) if int(flags) & 1 and int(flags) & 16}
        assert required <= actual, 'missing RPM config(noreplace) flags'
PY
    pass "arm64 packages installed with resolved runtime dependencies and stopped/disabled services"
    initialize
    check_invariants
    # Test both admin fail-close gates before enabling test tokens.
    cp -p /etc/edgion/controller/environment "$state/controller-environment"
    cp -p /etc/edgion/gateway/environment "$state/gateway-environment"
    : >/etc/edgion/controller/environment
    : >/etc/edgion/gateway/environment
    systemctl start edgion-controller edgion-gateway
    wait_until ready 12100
    wait_until curl -fsS --max-time 2 http://127.0.0.1:12000/health
    [[ $(curl -sS --max-time 2 -o /dev/null -w '%{http_code}' http://127.0.0.1:12101/api/v1/namespaced/Service/default) == 503 ]] || fail "Controller no-token gate"
    [[ $(curl -sS --max-time 2 -o /dev/null -w '%{http_code}' http://127.0.0.1:12001/configclient/Gateway/list) == 503 ]] || fail "Gateway no-token gate"
    systemctl stop edgion-gateway edgion-controller
    cp -p "$state/controller-environment" /etc/edgion/controller/environment
    cp -p "$state/gateway-environment" /etc/edgion/gateway/environment
    systemctl enable --now edgion-controller edgion-gateway
    wait_until ready 12100
    wait_until curl -fsS --max-time 2 http://127.0.0.1:12000/health
    python3 - "$state" <<'PY'
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import json, sys
for role, port in [('controller', 12101), ('gateway', 12001)]:
    path = '/api/v1/server-info' if role == 'controller' else '/configclient/Gateway/list'
    url = 'http://127.0.0.1:' + str(port) + path
    try:
        urlopen(url, timeout=5)
        raise AssertionError('unauthenticated business endpoint accepted')
    except HTTPError as error:
        assert error.code == 401, 'incorrect missing bearer response'
    token = Path(sys.argv[1], role + '-token').read_text()
    with urlopen(Request(url, headers={'Authorization': 'Bearer ' + token}), timeout=5) as response:
        assert response.status == 200, 'configured bearer rejected'
        if role == 'gateway':
            assert json.load(response).get('success') is True, 'Gateway list handler rejected request'
PY
    quiet_cli apply -f "$state/resources.yaml"
    wait_until fixture_ready
    # Listener ports are frozen at startup; apply the new port set by restart.
    systemctl restart edgion-gateway
    wait_until ready 12000
    wait_until proxy
    quiet_cli apply -f /tests/fixtures/crud.yaml
    quiet_cli get service package-crud -n default -o json
    grep -q package-crud "$state/cli-output" || fail "CLI read missing created resource"
    quiet_cli delete service package-crud -n default
    # Alpha input forces the installed config/crd lookup and strict source schemas.
    quiet_cli apply -f /tests/fixtures/schema.yaml
    quiet_cli get tcproute package-schema -n default -o json
    grep -q package-schema "$state/cli-output" || fail "installed alpha schema fixture missing"
    quiet_cli delete tcproute package-schema -n default
    pass "token-only auth, real CLI CRUD, mTLS sync and low-port proxy"
    wait_until bash -c 'find /var/lib/edgion/controller/resources -name "*.status" -type f | grep -q .'
    # Confirm deleted temporary CRUD resources have finished serial status cleanup.
    wait_until bash -c '! find /var/lib/edgion/controller/resources -type f \( -name "*package-crud*" -o -name "*package-schema*" \) | grep -q .'
    printf 'operator-status-preserved\n' >/var/lib/edgion/controller/resources/operator-state.status
    # Keep a matching non-resource file so startup orphan cleanup is not implicated.
    printf 'operator-record-preserved\n' >/var/lib/edgion/controller/resources/operator-state
    printf 'operator-data-preserved\n' >/var/lib/edgion/controller/resources/operator-data
    snapshot save
    gateway_pid=$(systemctl show edgion-gateway -p MainPID --value)
    systemctl stop edgion-controller
    [[ $(systemctl show edgion-gateway -p MainPID --value) == "$gateway_pid" ]] || fail "Controller restart replaced Gateway"
    proxy || fail "cached route failed while Controller was stopped"
    systemctl start edgion-controller
    wait_until ready 12100
    wait_until proxy
    pass "Controller restart keeps Gateway PID and existing traffic"
    rm -f /run/package-slow-started
    curl -fsS --max-time 20 -H 'Host: package.test' http://127.0.0.1/slow >"$state/slow-response" &
    slow_pid=$!
    wait_until test -f /run/package-slow-started
    stop_started=$SECONDS
    systemctl stop edgion-gateway
    wait "$slow_pid" || fail "in-flight request failed during graceful stop"
    [[ $(cat "$state/slow-response") == edgion-package-backend ]] || fail "in-flight response corrupted"
    (( SECONDS - stop_started < 90 )) || fail "graceful stop exceeded effective budget"
    [[ $(systemctl show edgion-gateway -p Result --value) == success ]] || fail "graceful stop required forced termination"
    systemctl start edgion-gateway
    wait_until proxy
    pass "SIGTERM drains a confirmed in-flight request inside stop budget"
    failure_recovery
    interrupted_hook_recovery
    native_policy_recovery
    deferred_mask_recovery
    install_set 2
    wait_until ready 12100
    wait_until proxy
    snapshot compare
    install_set 1
    wait_until ready 12100
    wait_until proxy
    snapshot compare
    pass "running upgrade and package rollback preserve configuration, credentials, data and overrides"
    # Explicitly stopped but still enabled, then disabled: neither may start.
    systemctl stop edgion-gateway edgion-controller
    install_set 2
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "stopped service started during upgrade"
        [[ $(systemctl is-enabled "edgion-$role") == enabled ]] || fail "upgrade lost enablement"
    done
    systemctl disable edgion-controller edgion-gateway
    install_set 1
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "disabled service started"
        [[ $(systemctl is-enabled "edgion-$role" || true) == disabled ]] || fail "upgrade changed disabled state"
    done
    systemctl mask edgion-controller edgion-gateway
    install_set 2
    for role in controller gateway; do
        [[ $(systemctl is-enabled "edgion-$role" || true) == masked ]] || fail "upgrade unmasked service"
        ! systemctl is-active --quiet "edgion-$role" || fail "masked service started"
    done
    systemctl unmask edgion-controller edgion-gateway
    snapshot compare
    pass "stopped, disabled and masked states preserved across native transactions"
    systemctl enable --now edgion-controller edgion-gateway
    wait_until ready 12100
    wait_until proxy
    pass "prepared enabled services for actual Docker container restart"
elif [[ "$phase" == finish ]]; then
    wait_until ready 12100
    wait_until ready 12000
    wait_until proxy
    snapshot compare
    pass "Docker restart preserves enablement and restored readiness/traffic"
    if [[ "$format" == deb ]]; then
        dpkg --remove edgion-gateway edgion-controller edgion-cli >"$state/package-output" 2>&1
    else
        rpm -e edgion-gateway edgion-controller edgion-cli >"$state/package-output" 2>&1
    fi
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "remove left daemon running"
        id "edgion-$role" >/dev/null
        [[ ! -e "/usr/bin/edgion-$role" ]] || fail "remove retained package binary"
    done
    snapshot removed
    pass "remove stops daemons and retains operator configuration/data/accounts"
    if [[ "$format" == deb ]]; then
        dpkg --purge edgion-gateway edgion-controller edgion-cli >"$state/package-output" 2>&1
        [[ ! -f /etc/edgion/controller/edgion-controller.yaml ]] || fail "DEB purge retained package conffile"
        [[ -f /etc/edgion/controller/certs/server.key ]] || fail "DEB purge removed operator credentials"
        [[ -f /etc/systemd/system/edgion-controller.service.d/operator.conf ]] || fail "DEB purge removed operator override"
        pass "native DEB config purge removes conffiles and retains operator credentials/overrides/data"
    else
        # RPM has no native purge. noreplace retains modified configs as .rpmsave.
        [[ -f /etc/edgion/controller/edgion-controller.yaml.rpmsave ]] || fail "RPM erase lost modified config"
        pass "RPM erase preserves modified config through rpmsave; no native purge exists"
    fi
    [[ -f /var/lib/edgion/controller/resources/operator-state.status ]] || fail "purge removed business status"
    [[ -f /var/lib/edgion/controller/resources/operator-data ]] || fail "purge removed business data"
    install_set 1
    for role in controller gateway; do
        ! systemctl is-active --quiet "edgion-$role" || fail "reinstall started daemon"
        [[ $(systemctl is-enabled "edgion-$role" || true) == disabled ]] || fail "reinstall enabled daemon"
    done
    # Explicitly restore saved process configs and tokens after native config removal.
    cp -p "$state/controller-config-backup" /etc/edgion/controller/edgion-controller.yaml
    cp -p "$state/controller-environment" /etc/edgion/controller/environment
    cp -p "$state/gateway-environment" /etc/edgion/gateway/environment
    cp -p "$state/gateway-config-backup" /etc/edgion/gateway/edgion-gateway.yaml
    systemctl start edgion-controller edgion-gateway
    wait_until ready 12100
    wait_until proxy
    check_invariants
    sysctl_snapshot >"$state/sysctl-after.json"
    cmp "$state/sysctl-before.json" "$state/sysctl-after.json" || fail "package lifecycle changed sysctls"
    pass "reinstall recovers retained business data and leaves namespace sysctls unchanged"
    # Explicit operator data purge is a separate, named action inside this container.
    systemctl stop edgion-gateway edgion-controller
    python3 - <<'PY'
from pathlib import Path
import shutil
backup = Path('/var/lib/package-test/operator-data-backup')
shutil.copy2('/var/lib/edgion/controller/resources/operator-data', backup)
for path in ['/var/lib/edgion/controller', '/var/lib/edgion/gateway',
             '/var/log/edgion/controller', '/var/log/edgion/gateway']:
    p = Path(path)
    assert p.is_dir() and not p.is_symlink(), 'unsafe test purge target'
    shutil.rmtree(p)
    assert not p.exists(), 'explicit data purge failed'
assert backup.read_text() == 'operator-data-preserved\n'
assert Path('/etc/edgion/controller/certs/server.key').is_file()
assert Path('/etc/edgion/gateway/certs/client.key').is_file()
PY
    id edgion-controller >/dev/null
    id edgion-gateway >/dev/null
    pass "explicit named data/log purge after backup leaves credentials and identities intact"
    python3 - "$state" "$format" <<'PY'
import json, pathlib, platform, sys
base = pathlib.Path(sys.argv[1])
report = {'status': 'passed', 'architecture': platform.machine(), 'format': sys.argv[2],
          'pid1': pathlib.Path('/proc/1/comm').read_text().strip(),
          'os_release': dict(line.split('=', 1) for line in pathlib.Path('/etc/os-release').read_text().splitlines() if '=' in line),
          'sysctls_unchanged': json.loads((base / 'sysctl-before.json').read_text()),
          'version_pair': '0.1.0-1 -> 0.1.0-2 -> 0.1.0-1 (same current-source binaries)',
          'limits': ['container restart is not host reboot', 'SELinux enforcement not verified',
                     'package rollback is not cross-product-version compatibility']}
(base / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
PY
else
    fail "unknown lifecycle phase"
fi
