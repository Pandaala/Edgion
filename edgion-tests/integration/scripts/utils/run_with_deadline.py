#!/usr/bin/env python3
"""Run one command in an owned process group with a hard deadline."""

import argparse
import os
import signal
import subprocess
import sys
import time
from typing import NoReturn


TIMEOUT_EXIT_CODE = 124
RESERVED_CHILD_EXIT_CODE = 125


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout-secs", type=int, required=True)
    parser.add_argument("--grace-secs", type=int, default=10)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.timeout_secs < 1:
        parser.error("--timeout-secs must be at least 1")
    if args.grace_secs < 1:
        parser.error("--grace-secs must be at least 1")
    if args.command and args.command[0] == "--":
        args.command = args.command[1:]
    if not args.command:
        parser.error("a command is required after --")
    return args


def group_has_live_members(pgid: int) -> bool:
    """Disambiguate EPERM without treating inaccessible live processes as gone."""
    result = subprocess.run(
        ["ps", "-axo", "pgid=,stat="],
        check=True,
        capture_output=True,
        text=True,
        timeout=1,
    )
    if not result.stdout.strip():
        raise ValueError("ps returned an empty process snapshot")
    for line in result.stdout.splitlines():
        group, state = line.split()
        if int(group) == pgid and not state.startswith("Z"):
            return True
    return False


def try_signal_group(process: subprocess.Popen, sig: int) -> bool:
    """Return false only when the owned group has no signalable live members."""
    try:
        os.killpg(process.pid, sig)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        # Darwin can return EPERM for an existing group containing only zombies.
        # Keep the leader unreaped until cleanup ends, retaining its PID ownership.
        if group_has_live_members(process.pid):
            raise
        return False


def signal_group(process: subprocess.Popen, sig: signal.Signals) -> None:
    try_signal_group(process, sig)


def process_group_exists(process: subprocess.Popen) -> bool:
    return try_signal_group(process, 0)


def terminate_group(process: subprocess.Popen, grace_secs: int) -> None:
    signal_group(process, signal.SIGTERM)
    grace_deadline = time.monotonic() + grace_secs
    while process_group_exists(process) and time.monotonic() < grace_deadline:
        time.sleep(0.05)
    if process_group_exists(process):
        signal_group(process, signal.SIGKILL)
    process.wait()


def main() -> int:
    args = parse_args()
    process = subprocess.Popen(args.command, start_new_session=True)

    def forward_signal(signum: int, _frame: object) -> NoReturn:
        terminate_group(process, args.grace_secs)
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, forward_signal)
    signal.signal(signal.SIGINT, forward_signal)

    try:
        exit_code = process.wait(timeout=args.timeout_secs)
        if exit_code == TIMEOUT_EXIT_CODE:
            print(
                f"child returned reserved timeout exit code {TIMEOUT_EXIT_CODE}; remapping to {RESERVED_CHILD_EXIT_CODE}",
                file=sys.stderr,
                flush=True,
            )
            return RESERVED_CHILD_EXIT_CODE
        return exit_code
    except subprocess.TimeoutExpired:
        print(
            f"deadline exceeded after {args.timeout_secs}s; terminating owned process group",
            file=sys.stderr,
            flush=True,
        )
        terminate_group(process, args.grace_secs)
        return TIMEOUT_EXIT_CODE


if __name__ == "__main__":
    raise SystemExit(main())
