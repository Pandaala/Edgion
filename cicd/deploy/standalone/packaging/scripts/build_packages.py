#!/usr/bin/env python3
"""Stage independent nFPM packages after validating the native ELF contract."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[5]
PACKAGING = Path(__file__).resolve().parents[1]
STANDALONE = PACKAGING.parent
NFPM_IMAGE = "goreleaser/nfpm:v2.47.0@sha256:a662cb167d7b6d3a83920c83d76b12d02b8ac5dd2c13e5c62c15270b23f6df0c"
LIBRARIES = {
    "libc.so.6": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "libm.so.6": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "libdl.so.2": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "libpthread.so.0": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "librt.so.1": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "libresolv.so.2": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "ld-linux-aarch64.so.1": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "ld-linux-x86-64.so.2": ("libc6 (>= 2.34)", "glibc >= 2.34"),
    "libgcc_s.so.1": ("libgcc-s1", "libgcc"),
    "libstdc++.so.6": ("libstdc++6", "libstdc++"),
    "libssl.so.3": ("libssl3", "openssl-libs"),
    "libcrypto.so.3": ("libssl3", "openssl-libs"),
    "libz.so.1": ("zlib1g", "zlib"),
    "libatomic.so.1": ("libatomic1", "libatomic"),
}


def elf_metadata(path, arch):
    data = path.read_bytes()
    if data[:6] != b"\x7fELF\x02\x01":
        raise ValueError(f"{path.name}: expected a little-endian ELF64 Linux executable")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    expected = {"arm64": 183, "amd64": 62}[arch]
    if header[1] not in (2, 3) or header[2] != expected:
        raise ValueError(f"{path.name}: ELF type or architecture does not match {arch}")
    section_offset, section_size, section_count = header[6], header[11], header[12]
    sections = [struct.unpack_from("<IIQQQQIIQQ", data, section_offset + i * section_size)
                for i in range(section_count)]
    needed, versions = set(), set()
    for section in sections:
        if section[1] != 6:  # SHT_DYNAMIC
            continue
        strings = sections[section[6]]
        string_data = data[strings[4]:strings[4] + strings[5]]
        versions.update(re.findall(rb"GLIBC_([0-9]+\.[0-9]+(?:\.[0-9]+)?)\x00", string_data))
        if b"GLIBC_PRIVATE\x00" in string_data:
            raise ValueError(f"{path.name}: private glibc ABI dependency")
        for offset in range(section[4], section[4] + section[5], section[9]):
            tag, value = struct.unpack_from("<qQ", data, offset)
            if tag == 1:  # DT_NEEDED
                needed.add(string_data[value:].split(b"\x00", 1)[0].decode("ascii"))
    if not needed or "libc.so.6" not in needed:
        raise ValueError(f"{path.name}: missing dynamic libc dependency")
    for version in versions:
        if tuple(map(int, version.decode().split("."))) > (2, 34):
            raise ValueError(f"{path.name}: requires GLIBC_{version.decode()}, above the Rocky 9 baseline")
    unknown = needed - LIBRARIES.keys()
    if unknown:
        raise ValueError(f"{path.name}: unmapped runtime dependencies: {sorted(unknown)}")
    return {"sha256": hashlib.sha256(data).hexdigest(), "architecture": arch,
            "needed": sorted(needed), "glibc_versions": sorted(v.decode() for v in versions)}


def content(stage, source, destination, mode=0o644, kind=None, group="root"):
    # Numbered staging filenames avoid collisions across similarly named docs.
    target = stage / "assets" / str(len(list((stage / "assets").iterdir())))
    shutil.copyfile(source, target)
    entry = {"src": "/stage/" + str(target.relative_to(stage)), "dst": destination,
             "file_info": {"mode": mode, "owner": "root", "group": group}}
    if kind:
        entry["type"] = kind
    return entry


def immutable_validation(contents, extra=()):
    files = sorted({item["dst"] for item in contents if item["dst"].startswith("/usr/")} | set(extra))
    directories = {str(parent) for file in files for parent in Path(file).parents if str(parent) != "/"}
    commands = [f"    immutable_directory {shlex.quote(path)}"
                for path in sorted(directories, key=lambda value: (len(Path(value).parts), value))]
    for path in files:
        quoted = shlex.quote(path)
        commands.extend([f"    if [ -e {quoted} ] || [ -L {quoted} ]; then",
                         f"        [ ! -L {quoted} ] && [ -f {quoted} ] || fail 'Unsafe immutable destination'",
                         f"        [ \"$(stat -c %u {quoted})\" = 0 ] || fail 'Non-root immutable destination'",
                         "    fi"])
    return "\n".join(commands)


def manifest(stage, name, format_name, args, metadata):
    spec = json.loads((PACKAGING / "nfpm" / f"{name}.json").read_text())
    format_config = spec.pop(format_name)
    format_dependencies = format_config["depends"]
    if format_config.get("predepends"):
        spec["deb"] = {"predepends": format_config["predepends"]}
    spec.pop("rpm" if format_name == "deb" else "deb", None)
    spec.update({"arch": args.arch, "platform": "linux", "version": args.version,
                 "release": args.release, "section": "net", "priority": "optional",
                 "maintainer": "Edgion maintainers <maintainers@edgion.io>",
                 "homepage": "https://github.com/Pandaala/Edgion", "license": "Apache-2.0"})
    index = 0 if format_name == "deb" else 1
    spec["depends"] = sorted(set(format_dependencies +
                                 [LIBRARIES[lib][index] for lib in metadata["needed"]]))
    contents = [content(stage, args.binaries / name, f"/usr/bin/{name}", 0o755),
                content(stage, ROOT / "LICENSE", f"/usr/share/doc/{name}/LICENSE"),
                content(stage, PACKAGING / "README.md", f"/usr/share/doc/{name}/packaging.md")]
    if name != "edgion-cli":
        role = name.removeprefix("edgion-")
        config_group = "root" if format_name == "deb" else name
        contents += [content(stage, STANDALONE / "systemd" / f"{name}.service",
                             f"/usr/lib/systemd/system/{name}.service"),
                     content(stage, PACKAGING / "config" / f"{name}.yaml",
                             f"/etc/edgion/{role}/{name}.yaml", 0o640, "config|noreplace", config_group),
                     content(stage, PACKAGING / "config" / f"{role}.environment",
                             f"/etc/edgion/{role}/environment", 0o640, "config|noreplace", config_group)]
        if role == "controller":
            crd_root = ROOT / "config" / "crd"
            for source in sorted(crd_root.rglob("*")):
                if source.is_file():
                    contents.append(content(stage, source,
                                            "/usr/share/edgion-controller/crd/" + str(source.relative_to(crd_root))))
        else:
            contents.append(content(stage, STANDALONE / "systemd" / "same-host.conf",
                                    f"/usr/share/doc/{name}/same-host.conf"))
        for source in sorted((STANDALONE.parent / "linux-tuning").rglob("*")):
            if source.is_file():
                contents.append(content(stage, source,
                    f"/usr/share/doc/{name}/linux-tuning/" + str(source.relative_to(STANDALONE.parent / "linux-tuning"))))
        common = (PACKAGING / "scripts" / "hooks" / "common.sh").read_text()
        common = common.replace("@ROLE@", role).replace("@FORMAT@", format_name)
        common = common.replace("@IMMUTABLE_LAYOUT@", immutable_validation(
            contents, [f"/usr/libexec/{name}/package-recover"]))
        recovery = stage / f"{name}-{format_name}-recover.sh"
        recovery.write_text("#!/bin/sh\n" + common + "\n" + """
case "${1:-}" in
    --cancel)
        if [ -e "$restart_state" ] || [ -L "$restart_state" ]; then
            validate_restart_state
            rm -f "$restart_state"
        fi
        if [ -e "$remove_state" ] || [ -L "$remove_state" ]; then
            validate_remove_state
            rm -f "$remove_state"
        fi
        ;;
    '')
        [ "${EDGION_PACKAGE_RECOVER:-}" = 1 ] || fail 'Set EDGION_PACKAGE_RECOVER=1 for explicit recovery'
        [ -f "/usr/bin/$account" ] && [ -f "/usr/lib/systemd/system/$unit" ] || fail 'Service package is not installed'
        layout configure
        restore_removal
        resume
        ;;
    *) fail 'Usage: package-recover [--cancel]' ;;
esac
""")
        contents.append(content(stage, recovery, f"/usr/libexec/{name}/package-recover", 0o755))
        spec["scripts"] = {}
        phases = ["preinstall", "postinstall", "preremove", "postremove"]
        if format_name == "rpm":
            phases.append("posttrans")
        for phase in phases:
            hook_path = stage / f"{name}-{format_name}-{phase}.sh"
            tail = (PACKAGING / "scripts" / "hooks" / f"{format_name}-{phase}.sh").read_text()
            hook_path.write_text("#!/bin/sh\n" + common + "\n" + tail)
            hook_path.chmod(0o755)
            hook_source = "/stage/" + hook_path.name
            if phase == "posttrans":
                spec["rpm"] = {"scripts": {"posttrans": hook_source}}
            else:
                spec["scripts"][phase] = hook_source
    spec["contents"] = contents
    if name == "edgion-cli":
        preinstall = stage / f"{name}-{format_name}-preinstall.sh"
        common = (PACKAGING / "scripts" / "hooks" / "common.sh").read_text()
        common = common.replace("@ROLE@", "cli").replace("@FORMAT@", format_name)
        common = common.replace("@IMMUTABLE_LAYOUT@", immutable_validation(contents))
        preinstall.write_text("#!/bin/sh\n" + common + "\nimmutable_layout\n")
        preinstall.chmod(0o755)
        spec["scripts"] = {"preinstall": "/stage/" + preinstall.name}
    return spec


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("arm64", "amd64"), default="arm64")
    parser.add_argument("--version", required=True)
    parser.add_argument("--release", default="1")
    parser.add_argument("--binaries", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--format", choices=("deb", "rpm", "all"), default="all")
    parser.add_argument("--stage-only", action="store_true", help="Validate and retain manifests without invoking Docker")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.version) or not re.fullmatch(r"[1-9][0-9]*", args.release):
        parser.error("version must be X.Y.Z and release a positive integer")
    args.binaries = args.binaries.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    metadata = {name: elf_metadata(args.binaries / name, args.arch)
                for name in ("edgion-controller", "edgion-gateway", "edgion-cli")}
    formats = ("deb", "rpm") if args.format == "all" else (args.format,)
    with tempfile.TemporaryDirectory(prefix="edgion-package-") as directory:
        stage = Path(directory)
        (stage / "assets").mkdir()
        for format_name in formats:
            for name in metadata:
                config_path = stage / f"{name}-{format_name}.json"
                spec = manifest(stage, name, format_name, args, metadata[name])
                config_path.write_text(json.dumps(spec, indent=2) + "\n")
                if not args.stage_only:
                    rpm_arch = {"arm64": "aarch64", "amd64": "x86_64"}[args.arch]
                    filename = (f"{name}_{args.version}-{args.release}_{args.arch}.deb" if format_name == "deb"
                                else f"{name}-{args.version}-{args.release}.{rpm_arch}.rpm")
                    subprocess.run(["docker", "run", "--rm",
                                    "--mount", f"type=bind,source={stage},target=/stage,readonly",
                                    "--mount", f"type=bind,source={args.output},target=/output",
                                    NFPM_IMAGE, "package", "--config", f"/stage/{config_path.name}",
                                    "--packager", format_name, "--target", f"/output/{filename}"], check=True)
        if args.stage_only:
            retained = args.output / f"stage-{args.version}-{args.release}"
            if retained.exists():
                raise ValueError(f"staging destination already exists: {retained}")
            shutil.copytree(stage, retained)
    (args.output / f"elf-{args.arch}-{args.version}-{args.release}.json").write_text(json.dumps(metadata, indent=2) + "\n")
    hashes = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
              for path in sorted(args.output.iterdir()) if path.suffix in (".deb", ".rpm")}
    (args.output / "checksums.json").write_text(json.dumps(hashes, indent=2) + "\n")


if __name__ == "__main__":
    main()
