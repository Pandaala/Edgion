#!/usr/bin/env python3
"""Exercise SDK selection without compiling guests or changing repository artifacts."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[3] / "edgion-tests/wasm/build-compatibility.sh"
FAKE_TOOL = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path

name = Path(sys.argv[0]).name
args = sys.argv[1:]
event = dict(tool=name, args=args, root=os.environ.get("GOROOT"),
             toolchain=os.environ.get("GOTOOLCHAIN"), path=os.environ["PATH"],
             goos=os.environ.get("GOOS"), goarch=os.environ.get("GOARCH"))
with open(os.environ["PROBE_LOG"], "a") as stream:
    stream.write(json.dumps(event) + "\n")
if args == ["env", "GOROOT"]:
    if name == "go":
        print(os.environ["EXPECTED_ROOT"])
    else:
        print("/wrong/sdk" if os.environ.get("WRONG_TINYGO_ROOT") else os.environ.get("GOROOT", "/ambient/sdk"))
elif args == ["version"] or args == ["--version"]:
    print(name + " fixture version")
elif args == ["mod", "verify"] or args[0] == "build":
    assert os.environ["GOROOT"] == os.environ["EXPECTED_ROOT"]
    assert os.environ["GOTOOLCHAIN"] == "local"
    assert os.environ["PATH"].split(os.pathsep)[0] == os.environ["EXPECTED_ROOT"] + "/bin"
else:
    raise SystemExit("unexpected tool invocation")
'''


class ToolchainSelectionTest(unittest.TestCase):
    def run_probe(self, wrong_root=False, named_tools=False):
        with tempfile.TemporaryDirectory(prefix="wasm sdk selection ") as directory:
            root = Path(directory).resolve()
            sdk = root / "selected sdk"
            tools = root / "compiler tools"
            ambient = root / "ambient"
            fixture = root / "fixture"
            for path in (sdk / "bin", tools, ambient, fixture / "tinygo-policy", fixture / "go-reactor-probe"):
                path.mkdir(parents=True)
            for path in (sdk / "bin/go", tools / "tinygo", tools / "wasm-opt"):
                path.write_text(FAKE_TOOL)
                path.chmod(0o755)
            # Any accidental discovery of the ambient Go must fail the test.
            (ambient / "go").write_text("#!/bin/sh\nexit 97\n")
            (ambient / "go").chmod(0o755)
            shutil.copy2(SCRIPT, fixture / "build-compatibility.sh")
            log = root / "calls.jsonl"
            env = os.environ.copy()
            env.update(
                GO_BIN="./selected sdk/bin/go",
                TINYGO_BIN="./compiler tools/tinygo",
                WASMOPT="./compiler tools/wasm-opt",
                GOROOT="/invalid/inherited/root",
                GOTOOLCHAIN="go9.99.0+auto",
                EXPECTED_ROOT=str(sdk),
                PROBE_LOG=str(log),
                PATH=str(ambient) + os.pathsep + env["PATH"],
            )
            env.pop("WRONG_TINYGO_ROOT", None)
            if wrong_root:
                env["WRONG_TINYGO_ROOT"] = "1"
            if named_tools:
                env.update(GO_BIN="go", TINYGO_BIN="tinygo", WASMOPT="wasm-opt")
                env["PATH"] = os.pathsep.join((str(sdk / "bin"), str(tools), env["PATH"]))
            result = subprocess.run(
                ["bash", "fixture/build-compatibility.sh"], cwd=root, env=env,
                text=True, capture_output=True, check=False,
            )
            events = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertEqual(events[0]["tool"], "go")
            self.assertEqual(events[0]["args"], ["env", "GOROOT"])
            self.assertIsNone(events[0]["root"], "ambient GOROOT must not affect SDK discovery")
            self.assertTrue(all(event["toolchain"] == "local" for event in events))
            builds = [event for event in events if event["args"][0] == "build"]
            if wrong_root:
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("TinyGo did not select the requested Go SDK", result.stderr)
                self.assertEqual(builds, [])
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual([event["tool"] for event in builds], ["tinygo", "go"])
                self.assertTrue(all(event["root"] == str(sdk) for event in events[1:]))
                self.assertEqual(builds[1]["goos"], "wasip1")
                self.assertEqual(builds[1]["goarch"], "wasm")

    def test_relative_overrides_with_spaces_replace_ambient_sdk(self):
        self.run_probe()

    def test_named_tools_resolve_before_path_change(self):
        self.run_probe(named_tools=True)

    def test_wrong_tinygo_sdk_fails_before_build(self):
        self.run_probe(wrong_root=True)


if __name__ == "__main__":
    unittest.main()
