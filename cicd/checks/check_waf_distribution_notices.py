#!/usr/bin/env python3
"""Compare shipped native WAF notices with their canonical source bytes.

Use --directory for extracted packages or binary bundles, --archive for a tar
distribution, or --image for a built Docker image (its entrypoint is never run).
Use --prefix to select the notice directory inside the artifact. Add new notice
files to edgion-waf/licenses and include them in each packaging path; new formats
must expose their extracted contents to this check before publication.
"""

import argparse
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def expected_files():
    waf = ROOT / "edgion-waf"
    for name in ("libinjection-go-BSD-3-Clause.txt", "libinjectionrs-BSD-3-Clause.txt"):
        if not (waf / "licenses" / name).is_file():
            raise ValueError(f"Canonical WAF license is missing: {name}")
    return {"INJECTION.md": (waf / "INJECTION.md").read_bytes(),
            **{f"licenses/{p.name}": p.read_bytes() for p in sorted((waf / "licenses").glob("*.txt"))}}


def verify(read, prefix):
    for name, expected in expected_files().items():
        path = "/".join(part for part in (prefix.strip("/"), name) if part)
        if read(path) != expected:
            raise ValueError(f"Missing or modified WAF notice: {path}")


def check_archive(path, prefix):
    with tarfile.open(path) as archive:
        members = {m.name.removeprefix("./"): m for m in archive.getmembers()}

        def read(name):
            member = members.get(name)
            if member is None or not member.isfile():
                return None
            with archive.extractfile(member) as stream:
                return stream.read()

        verify(read, prefix)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--directory", type=Path)
    source.add_argument("--archive", type=Path)
    source.add_argument("--image")
    parser.add_argument("--prefix", default="usr/share/doc/edgion/waf")
    args = parser.parse_args()
    if args.directory:
        def read(name):
            path = args.directory / name
            return path.read_bytes() if path.is_file() else None
        verify(read, args.prefix)
    elif args.archive:
        check_archive(args.archive, args.prefix)
    else:
        container = subprocess.check_output(
            ["docker", "create", "--entrypoint", "/bin/true", args.image], text=True).strip()
        try:
            with tempfile.TemporaryDirectory(prefix="edgion-waf-notices-") as directory:
                subprocess.run(["docker", "cp", f"{container}:/{args.prefix.strip('/')}/.", directory], check=True)
                verify(lambda name: (Path(directory) / name).read_bytes()
                       if (Path(directory) / name).is_file() else None, "")
        finally:
            subprocess.run(["docker", "rm", container], check=True, stdout=subprocess.DEVNULL)
    print("WAF distribution notices match the canonical source files.")


if __name__ == "__main__":
    main()
