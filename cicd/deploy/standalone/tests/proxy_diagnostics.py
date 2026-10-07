"""Bounded, allowlisted snapshots after a failed package proxy assertion."""
import ipaddress
import json
import os
import signal
import stat
import subprocess
import time
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


TOTAL_BUDGET_SECONDS = 15
PROBE_TIMEOUT_SECONDS = 2
FILE_CAP = 65536
UNAVAILABLE = "unavailable"
# These literals are verified wire_code values in edgion-resources/src/diagnostic.rs.
# Unrecognized codes are represented by a fixed placeholder, never reflected.
ERROR_CODES = frozenset({
    "Unknown", "HostMissing", "RouteNotFound", "UpstreamNotRouteMatched",
    "UpstreamNotBackendRefs", "UpstreamInconsistentWeight",
    "BackendEndpointSliceNotFoundByConsistent", "BackendEndpointSliceNotFoundByLeastConn",
    "BackendEndpointSliceNotFoundByRoundRobin", "BackendEndpointSliceNotFoundByRoundRobinDefault",
    "BackendEndpointSliceNotFoundByEwma", "BackendPortResolutionFailed",
    "BackendAddressParsingFailed", "BackendLocalhostNotAllowed", "BackendAddrLinkLocalNotAllowed",
    "BackendServiceNotFound", "UpstreamTransportRetryCeiling", "RequestTimeoutExceeded", "ClientClosed",
})
UNIT_FIELDS = {
    "ActiveState": frozenset({"active", "reloading", "inactive", "failed", "activating", "deactivating", "maintenance", "refreshing"}),
    "SubState": frozenset({"running", "dead", "failed", "start", "start-pre", "start-post", "auto-restart", "stop", "stop-sigterm", "stop-sigkill", "stop-post", "final-sigterm", "final-sigkill", "exited", "reload", "reload-signal", "cleaning"}),
    "Result": frozenset({"success", "resources", "protocol", "timeout", "exit-code", "signal", "core-dump", "watchdog", "start-limit-hit", "oom-kill", "exec-condition", "skip-condition"}),
}
UNITS = ("package-backend.service", "edgion-controller.service", "edgion-gateway.service")


class DeadlineReached(Exception):
    pass


class UnsafeFile(Exception):
    pass


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, _request, _fp, _code, _message, _headers, _url):
        return None


def remaining(deadline, maximum=PROBE_TIMEOUT_SECONDS):
    left = deadline - time.monotonic()
    if left <= 0:
        raise DeadlineReached()
    return min(maximum, left)


def read_regular(path, maximum, tail=False):
    flags = os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC
    fd = os.open(path, flags)
    try:
        size = os.fstat(fd)
        if not stat.S_ISREG(size.st_mode):
            raise UnsafeFile()
        if tail:
            start = max(0, size.st_size - maximum)
            os.lseek(fd, start, os.SEEK_SET)
            data = os.read(fd, maximum)
            # Discard a potentially partial first record without another read.
            return data.partition(b"\n")[2] if start else data
        data = os.read(fd, maximum + 1)
        if len(data) > maximum:
            raise UnsafeFile()
        return data
    finally:
        os.close(fd)


def backend_ipv4(path):
    try:
        address = ipaddress.IPv4Address(read_regular(path, 32).decode("ascii").strip())
        if address.is_loopback or address.is_link_local or address.is_unspecified or address.is_multicast:
            return None
        return address
    except DeadlineReached:
        raise
    except Exception:
        return None


def run_command(args, deadline, maximum=FILE_CAP):
    try:
        result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=remaining(deadline), check=False)
        if result.returncode != 0 or len(result.stdout) > maximum:
            return None
        return result.stdout.decode("utf-8", errors="strict")
    except DeadlineReached:
        raise
    except Exception:
        return None


def address_in_namespace(address, deadline):
    if address is None:
        return UNAVAILABLE
    output = run_command(["ip", "-j", "-4", "addr", "show", "scope", "global"], deadline)
    if output is None:
        return UNAVAILABLE
    try:
        interfaces = json.loads(output)
        return any(ipaddress.IPv4Address(item["local"]) == address
                   for interface in interfaces if interface.get("ifname") != "lo"
                   for item in interface.get("addr_info", []) if item.get("family") == "inet")
    except DeadlineReached:
        raise
    except Exception:
        return UNAVAILABLE


def http_status(url, deadline, proxy=False):
    # No environment proxy, redirects, credential headers or body reads.
    request = Request(url, headers={"Host": "package.test"} if proxy else {})
    opener = build_opener(ProxyHandler({}), NoRedirect())
    response = None
    try:
        response = opener.open(request, timeout=remaining(deadline))
        status = response.status
        return status if type(status) is int and 100 <= status <= 599 else UNAVAILABLE
    except HTTPError as error:
        response = error
        return error.code if type(error.code) is int and 100 <= error.code <= 599 else UNAVAILABLE
    except DeadlineReached:
        raise
    except Exception:
        return "transport_failure"
    finally:
        if response is not None:
            response.close()


def unit_properties(output):
    fields = {name: UNAVAILABLE for name in tuple(UNIT_FIELDS) + ("ExecMainStatus",)}
    if output is None or len(output) > 4096:
        return fields
    for line in output.splitlines():
        name, separator, value = line.partition("=")
        if not separator:
            continue
        if name in UNIT_FIELDS:
            fields[name] = value if value in UNIT_FIELDS[name] else UNAVAILABLE
        elif name == "ExecMainStatus":
            if value.isascii() and value.isdecimal() and len(value) <= 3:
                number = int(value)
                fields[name] = number if 0 <= number <= 255 else UNAVAILABLE
    return fields


def access_records(path):
    try:
        data = read_regular(path, FILE_CAP, tail=True)
        records = []
        for line in data.splitlines()[-5:]:
            try:
                record = json.loads(line)
                request = record.get("request_info", {})
                status = request.get("status") if isinstance(request, dict) else None
                status = status if type(status) is int and 100 <= status <= 599 else UNAVAILABLE
                codes = []
                diagnostics = record.get("diagnostics", [])
                for item in diagnostics[:32] if isinstance(diagnostics, list) else []:
                    if isinstance(item, dict) and item.get("kind") == "error":
                        code = item.get("code")
                        codes.append(code if isinstance(code, str) and code in ERROR_CODES else "other_error_code")
                records.append({"status": status, "error_codes": codes})
            except DeadlineReached:
                raise
            except Exception:
                continue
        return records
    except DeadlineReached:
        raise
    except Exception:
        return UNAVAILABLE


def collect_snapshot(deadline):
    address = backend_ipv4("/var/lib/package-test/backend-ipv4")
    membership = address_in_namespace(address, deadline)
    probes = {
        "controller_ready": "http://127.0.0.1:12100/ready",
        "gateway_ready": "http://127.0.0.1:12000/ready",
        "backend_loopback": "http://127.0.0.1:18080/",
        "gateway_proxy": "http://127.0.0.1/",
    }
    if membership is True:
        probes["backend_non_loopback"] = "http://" + str(address) + ":18080/"
    statuses = {name: http_status(url, deadline, proxy=name == "gateway_proxy") for name, url in probes.items()}
    units = {unit: unit_properties(run_command(
        ["systemctl", "show", unit, "--property=ActiveState,SubState,Result,ExecMainStatus"], deadline, 4096))
        for unit in UNITS}
    remaining(deadline)
    return {"http": statuses, "backend_address_in_namespace": membership, "units": units,
            "access_log": access_records("/var/log/edgion/gateway/edgion_access.log")}


def main(timeout_seconds=TOTAL_BUDGET_SECONDS):
    def alarm(_signal, _frame):
        raise DeadlineReached()

    signal.signal(signal.SIGALRM, alarm)
    signal.setitimer(signal.ITIMER_REAL, timeout_seconds)
    try:
        try:
            result = {"proxy_timeout_diagnostics": collect_snapshot(time.monotonic() + timeout_seconds)}
        except Exception:
            result = {"proxy_timeout_diagnostics": UNAVAILABLE}
        print(json.dumps(result, separators=(",", ":")), flush=True)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)


if __name__ == "__main__":
    main()
