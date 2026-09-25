#!/usr/bin/env python3
"""Validate fixed isolated ports and probe their host-side TCP availability."""

import argparse
from pathlib import Path
import shutil
import socket
import subprocess
import sys


PORTS_FILE = Path(__file__).with_name("isolated_ports.sh")


def shell_output(argument: str) -> str:
    result = subprocess.run(
        ["bash", str(PORTS_FILE), argument],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append", default=[])
    parser.add_argument("--definitions-only", action="store_true")
    args = parser.parse_args()

    try:
        scenarios = shell_output("--list-scenarios").splitlines()
        if not scenarios or len(scenarios) != len(set(scenarios)):
            raise ValueError("isolated scenario registry is empty or duplicated")
        selected = set(args.scenario or scenarios)
        if unknown := selected.difference(scenarios):
            raise ValueError(f"unknown isolated scenarios: {', '.join(sorted(unknown))}")

        ports: dict[int, tuple[str, str, str, str]] = {}
        for scenario in scenarios:
            rows = shell_output(scenario).splitlines()
            if not rows:
                raise ValueError(f"{scenario} has no registered ports")
            for row in rows:
                owner, name, raw_port, bind, policy = row.split("\t")
                if owner != scenario:
                    raise ValueError(f"unexpected port owner: {row}")
                port = int(raw_port)
                if previous := ports.get(port):
                    raise ValueError(
                        f"port {port} is assigned to both {previous[0]}/{previous[1]} "
                        f"and {scenario}/{name}"
                    )
                ports[port] = (scenario, name, bind, policy)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(f"ERROR: isolated port definitions are invalid: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.rstrip(), file=sys.stderr)
        return 1

    if args.definitions_only:
        print(f"PASS: {len(ports)} fixed isolated port assignments are unique")
        return 0

    conflicts = 0
    checked = 0
    for port, (scenario, name, bind, policy) in sorted(ports.items()):
        if scenario not in selected or bind == "docker":
            continue
        checked += 1
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                if policy == "reuse":
                    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                probe.bind((bind, port))
                probe.listen(1)
        except OSError as error:
            conflicts += 1
            print(
                f"ERROR: {scenario}/{name} cannot bind {bind}:{port}: {error}",
                file=sys.stderr,
            )
            if shutil.which("lsof"):
                owner = subprocess.run(
                    ["lsof", "-nP", f"-iTCP:{port}"], capture_output=True, text=True
                )
                if owner.stdout:
                    print(owner.stdout.rstrip(), file=sys.stderr)

    if conflicts:
        return 1
    print(f"PASS: {checked} fixed isolated host TCP ports are available")
    return 0


if __name__ == "__main__":
    sys.exit(main())
