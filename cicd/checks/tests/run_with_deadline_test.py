#!/usr/bin/env python3
"""Focused process-lifecycle checks; run directly on Darwin and Linux."""

import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[3]
RUNNER = ROOT / "edgion-tests/integration/scripts/utils/run_with_deadline.py"
spec = importlib.util.spec_from_file_location("deadline", RUNNER)
deadline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deadline)


class ProbeTests(unittest.TestCase):
    def test_permission_error_requires_no_live_members(self):
        process = mock.Mock(pid=123)
        for sig in (0, signal.SIGTERM, signal.SIGKILL):
            for rows, expected in (
                ("123 Z\n456 S\n", False),
                ("123 Z\n123 S\n", True),
                ("456 S\n", False),
            ):
                with self.subTest(sig=sig, rows=rows), mock.patch.object(
                    deadline.os, "killpg", side_effect=PermissionError(1, "denied")
                ), mock.patch.object(
                    deadline.subprocess, "run", return_value=mock.Mock(stdout=rows)
                ):
                    if expected:
                        with self.assertRaises(PermissionError):
                            deadline.try_signal_group(process, sig)
                    else:
                        self.assertFalse(deadline.try_signal_group(process, sig))

    def test_failed_or_malformed_snapshot_is_not_success(self):
        with mock.patch.object(deadline.os, "killpg", side_effect=PermissionError(1, "denied")):
            for error in (subprocess.TimeoutExpired("ps", 1), subprocess.CalledProcessError(1, "ps")):
                with self.subTest(error=error), mock.patch.object(deadline.subprocess, "run", side_effect=error):
                    with self.assertRaises(type(error)):
                        deadline.process_group_exists(mock.Mock(pid=123))
            for rows in ("invalid\n", ""):
                with mock.patch.object(deadline.subprocess, "run", return_value=mock.Mock(stdout=rows)):
                    with self.assertRaises(ValueError):
                        deadline.process_group_exists(mock.Mock(pid=123))


class LifecycleTests(unittest.TestCase):
    def run_command(self, code):
        return subprocess.run(
            [sys.executable, str(RUNNER), "--timeout-secs", "1", "--grace-secs", "1",
             "--", sys.executable, "-c", code],
            capture_output=True, text=True, timeout=8,
        )

    def test_child_exit_codes(self):
        for child, expected in ((0, 0), (7, 7), (124, 125)):
            with self.subTest(child=child):
                self.assertEqual(self.run_command(f"raise SystemExit({child})").returncode, expected)

    def test_timeout(self):
        result = self.run_command("import time; time.sleep(30)")
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_term_resistant_child(self):
        result = self.run_command(
            "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
        )
        self.assertEqual(result.returncode, 124, result.stderr)

    def test_descendant_outlives_leader(self):
        # Keep the child in the original session/group; its TERM handler is ignored.
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            child = (
                "import os, signal, time; from pathlib import Path; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                f"Path({str(ready)!r}).write_text(str(os.getpid())); time.sleep(30)"
            )
            leader = f"import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', {child!r}]); time.sleep(30)"
            try:
                result = self.run_command(leader)
                self.assertEqual(result.returncode, 124, result.stderr)
                self.assertTrue(ready.exists(), "descendant did not become ready")
                pid = int(ready.read_text())
                end = time.monotonic() + 2
                while time.monotonic() < end:
                    snapshot = subprocess.run(
                        ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, timeout=1,
                    )
                    if snapshot.returncode == 1 or snapshot.stdout.strip().startswith("Z"):
                        break
                    self.assertEqual(snapshot.returncode, 0, snapshot.stderr)
                    time.sleep(0.02)
                else:
                    self.fail("descendant survived group cleanup")
            finally:
                if ready.exists():
                    try:
                        os.kill(int(ready.read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_unreaped_leader(self):
        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        try:
            end = time.monotonic() + 2
            while time.monotonic() < end:
                state = subprocess.check_output(["ps", "-o", "stat=", "-p", str(process.pid)], text=True)
                if state.strip().startswith("Z"):
                    break
                time.sleep(0.02)
            else:
                self.fail("child did not reach unreaped state")
            deadline.terminate_group(process, 1)
            self.assertEqual(process.returncode, 0)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


if __name__ == "__main__":
    unittest.main()
