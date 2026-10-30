#!/usr/bin/env python3
"""Exercise the local Controller owner with real disposable subprocesses."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


HELPER = Path(__file__).resolve().parents[3] / "edgion-tests/integration/scripts/utils/restart_controller.py"


class RestartControllerTest(unittest.TestCase):
    def test_stop_during_restart_prevents_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory).resolve()
            (work / "pids").mkdir()
            fake = work / "edgion-controller"
            fake.write_text("#!" + sys.executable + "\n" + '''
import os, signal, sys, time
from pathlib import Path
def stop(*_):
    Path("stopping").touch()
    time.sleep(1)
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
with Path("starts").open("a") as stream:
    stream.write(str(os.getpid()) + "\\n")
while True:
    signal.pause()
''')
            fake.chmod(0o700)
            owner = subprocess.Popen(
                [sys.executable, str(HELPER), "--start", str(work), str(fake), "--work-dir", str(work)],
                cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 10
                while not (work / "starts").exists() or not (work / "starts").read_text().strip():
                    if owner.poll() is not None or time.monotonic() >= deadline:
                        self.fail("Controller did not start")
                    time.sleep(0.01)
                child_pid = int((work / "starts").read_text().strip())
                os.kill(owner.pid, signal.SIGUSR1)
                while not (work / "stopping").exists():
                    if owner.poll() is not None or time.monotonic() >= deadline:
                        self.fail("Controller did not begin restart shutdown")
                    time.sleep(0.01)
                subprocess.run([sys.executable, str(HELPER), "--stop", str(work)],
                               check=True, capture_output=True, timeout=10)
                self.assertEqual(owner.wait(timeout=10), 0)
                self.assertEqual((work / "starts").read_text().splitlines(), [str(child_pid)])
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)
                self.assertTrue(json.loads((work / "pids/controller-state.json").read_text())["stopped"])
                subprocess.run([sys.executable, str(HELPER), "--stop", str(work)],
                               check=True, capture_output=True, timeout=10)
            finally:
                if owner.poll() is None:
                    owner.terminate()
                    owner.wait(timeout=10)
                owner.stderr.close()

    def test_restart_preserves_launch_and_never_overlaps_or_restarts_gateway(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory).resolve()
            (work / "pids").mkdir()
            fake = work / "edgion-controller"
            fake.write_text("#!" + sys.executable + "\n" + '''
import json, os, signal, sys, time
from pathlib import Path
def record(event):
    with Path("events.jsonl").open("a") as stream:
        stream.write(json.dumps({"event": event, "pid": os.getpid(), "argv": sys.argv,
                                 "env": os.environ["OWNER_TEST_SECRET"], "cwd": os.getcwd()}) + "\\n")
def stop(*_):
    time.sleep(0.2)
    record("stop")
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
record("start")
while True:
    signal.pause()
''')
            fake.chmod(0o700)
            argv = [str(fake), "--work-dir", str(work), "--opaque", "argument with spaces"]
            env = dict(os.environ, OWNER_TEST_SECRET="original-private-value")
            owner = subprocess.Popen([sys.executable, str(HELPER), "--start", str(work), *argv],
                                     cwd=work, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            gateway = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
            state = work / "pids/controller-state.json"

            def wait_for(predicate):
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if predicate():
                        return
                    if owner.poll() is not None:
                        self.fail(owner.stderr.read().decode())
                    time.sleep(0.05)
                self.fail("subprocess condition timed out")

            def events():
                path = work / "events.jsonl"
                return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

            try:
                wait_for(lambda: len(events()) == 1 and state.exists())
                initial = json.loads(state.read_text())
                # A stale/forged marker must fail without signaling any process.
                marker = work / "pids/controller-owner.json"
                original_marker = marker.read_text()
                invalid = json.loads(original_marker)
                invalid["identity"] = "wrong identity"
                marker.write_text(json.dumps(invalid))
                denied = subprocess.run([sys.executable, str(HELPER), str(work)], capture_output=True, timeout=10)
                self.assertNotEqual(denied.returncode, 0)
                self.assertEqual(initial, json.loads(state.read_text()))
                marker.write_text(original_marker)
                subprocess.run([sys.executable, str(HELPER), str(work)],
                               env=dict(os.environ, OWNER_TEST_SECRET="different-caller-value"),
                               check=True, capture_output=True, timeout=10)
                wait_for(lambda: len(events()) == 3)
                history = events()
                self.assertEqual([item["event"] for item in history], ["start", "stop", "start"])
                self.assertEqual(history[0]["pid"], history[1]["pid"])
                self.assertNotEqual(history[0]["pid"], history[2]["pid"])
                self.assertEqual(history[0]["argv"], history[2]["argv"])
                self.assertEqual(history[2]["argv"], argv)
                self.assertEqual(history[2]["env"], "original-private-value")
                self.assertEqual(history[2]["cwd"], str(work))
                self.assertIsNone(gateway.poll())
                self.assertNotIn("original-private-value", "".join(p.read_text() for p in (work / "pids").iterdir()))
                # Existing stop_all kills the child; that must also end the owner.
                os.kill(history[2]["pid"], signal.SIGTERM)
                self.assertEqual(owner.wait(timeout=10), 0)
                self.assertTrue(json.loads(state.read_text())["stopped"])
            finally:
                if owner.poll() is None:
                    owner.terminate()
                    owner.wait(timeout=10)
                owner.stderr.close()
                gateway.terminate()
                gateway.wait(timeout=10)


if __name__ == "__main__":
    unittest.main()
