#!/usr/bin/env python3
"""Own and restart one integration Controller without persisting credentials.

The launch owner retains the original argv, environment and working directory in
memory. SIGUSR1 requests a serial stop/wait/start; killing the Controller normally
also ends its owner. Gateway and test_server are never touched.
"""

import json
import fcntl
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def atomic_write(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(value)
    temporary.replace(path)


def process_identity(pid):
    return subprocess.check_output(
        ["ps", "-ww", "-p", str(pid), "-o", "lstart=", "-o", "command="], text=True
    ).strip()


def own(work, argv):
    if not argv or Path(argv[0]).name != "edgion-controller":
        raise RuntimeError("Expected an edgion-controller executable")
    if "--work-dir" not in argv or Path(argv[argv.index("--work-dir") + 1]).resolve() != work:
        raise RuntimeError("Controller arguments do not own this work directory")
    pid_path = work / "pids/controller.pid"
    owner_path = work / "pids/controller-owner.json"
    state_path = work / "pids/controller-state.json"
    # Hold the lock for this owner's entire lifetime, including child shutdown.
    owner_lock = (work / "pids/controller-owner.lock").open("w")
    fcntl.flock(owner_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    restart_requested = False
    stopping = False
    restarting = False

    def request_restart(_signum, _frame):
        nonlocal restart_requested
        if not restarting and not stopping:
            restart_requested = True

    def request_stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGUSR1, request_restart)
    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    atomic_write(state_path, json.dumps({"starting": True}))
    atomic_write(owner_path, json.dumps({"pid": os.getpid(), "identity": process_identity(os.getpid())}))
    child = None
    generation = 0
    try:
        while not stopping:
            child = subprocess.Popen(argv, stdin=subprocess.DEVNULL)
            generation += 1
            atomic_write(pid_path, f"{child.pid}\n")
            atomic_write(state_path, json.dumps({"pid": child.pid, "owner_pid": os.getpid(), "generation": generation}))
            restarting = False
            while child.poll() is None and not restart_requested and not stopping:
                time.sleep(0.05)
            if stopping or not restart_requested:
                break
            restart_requested = False
            restarting = True
            child.terminate()
            # Do not start a replacement until the original process is reaped.
            child.wait(timeout=30)
    finally:
        if child is not None and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
        atomic_write(state_path, json.dumps({"stopped": True}))


def owner_pid(work):
    owner = json.loads((work / "pids/controller-owner.json").read_text())
    pid = owner["pid"]
    if not isinstance(pid, int) or pid <= 1:
        raise RuntimeError("Invalid Controller owner state")
    identity = process_identity(pid)
    if identity != owner["identity"] or str(work) not in identity or "--start" not in identity:
        raise RuntimeError("Controller owner PID no longer belongs to this invocation")
    return pid


def stop(work):
    if not (work / "pids/controller-owner.json").exists():
        return
    try:
        pid = owner_pid(work)
    except subprocess.CalledProcessError:
        # The recorded owner has already exited.
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 70
    state_path = work / "pids/controller-state.json"
    while time.monotonic() < deadline:
        if state_path.exists() and json.loads(state_path.read_text()).get("stopped"):
            return
        time.sleep(0.05)
    raise RuntimeError("Controller owner did not stop within 70 seconds")


def restart(work):
    state_path = work / "pids/controller-state.json"
    before = json.loads(state_path.read_text())
    if "generation" not in before:
        raise RuntimeError("Invalid Controller owner state")
    pid = owner_pid(work)
    os.kill(pid, signal.SIGUSR1)
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        current = json.loads(state_path.read_text())
        if current.get("stopped"):
            raise RuntimeError("Controller owner stopped without a replacement")
        if current["generation"] > before["generation"]:
            print(f"Controller replaced: {before['pid']} -> {current['pid']}")
            return
        time.sleep(0.05)
    raise RuntimeError("Controller replacement did not complete within 45 seconds")


if __name__ == "__main__":
    if len(sys.argv) > 3 and sys.argv[1] == "--start":
        own(Path(sys.argv[2]).resolve(strict=True), sys.argv[3:])
    elif len(sys.argv) == 3 and sys.argv[1] == "--stop":
        stop(Path(sys.argv[2]).resolve(strict=True))
    elif len(sys.argv) == 2:
        restart(Path(sys.argv[1]).resolve(strict=True))
    else:
        raise SystemExit("Usage: restart_controller.py [--start|--stop] WORK_DIR [CONTROLLER ARGS...]")
