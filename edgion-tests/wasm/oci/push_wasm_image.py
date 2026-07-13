#!/usr/bin/env python3
"""Push a wasm module to an OCI registry as a wasm image (both variants).

oci variant    - raw wasm layer, media type
                 application/vnd.module.wasm.content.layer.v1+wasm
compat variant - single tar.gz layer containing plugin.wasm

Usage:
  python3 push_wasm_image.py --registry localhost:5000 \
      --repo wasm/header-demo --tag 1.0.0 \
      --wasm ../artifacts/header_demo.wasm --variant oci \
      [--username u --password p]
"""
import argparse
import base64
import gzip
import hashlib
import io
import json
import tarfile
import urllib.error
import urllib.request


def sha256_hex(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def request(method, url, data=None, headers=None, auth=None):
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    if auth:
        token = base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode()
        req.add_header("Authorization", f"Basic {token}")
    try:
        with urllib.request.urlopen(req) as resp:
            return resp.status, dict(resp.headers), resp.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def upload_blob(base, repo, blob, auth):
    digest = "sha256:" + sha256_hex(blob)
    status, headers, body = request("POST", f"{base}/v2/{repo}/blobs/uploads/", auth=auth)
    assert status == 202, f"blob upload start failed: {status} {body!r}"
    location = headers["Location"]
    if location.startswith("/"):
        location = base + location
    sep = "&" if "?" in location else "?"
    status, _, body = request(
        "PUT",
        f"{location}{sep}digest={digest}",
        data=blob,
        headers={"Content-Type": "application/octet-stream"},
        auth=auth,
    )
    assert status == 201, f"blob PUT failed: {status} {body!r}"
    return digest, len(blob)


def build_compat_layer(wasm: bytes) -> bytes:
    tar_buf = io.BytesIO()
    with tarfile.open(fileobj=tar_buf, mode="w") as tf:
        info = tarfile.TarInfo("plugin.wasm")
        info.size = len(wasm)
        tf.addfile(info, io.BytesIO(wasm))
    return gzip.compress(tar_buf.getvalue())


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--registry", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--wasm", required=True)
    p.add_argument("--variant", choices=["oci", "compat"], default="oci")
    p.add_argument("--username")
    p.add_argument("--password")
    args = p.parse_args()

    auth = (args.username, args.password) if args.username else None
    base = f"http://{args.registry}"
    with open(args.wasm, "rb") as f:
        wasm = f.read()

    if args.variant == "oci":
        layer, layer_media = wasm, "application/vnd.module.wasm.content.layer.v1+wasm"
    else:
        layer = build_compat_layer(wasm)
        layer_media = "application/vnd.oci.image.layer.v1.tar+gzip"

    config = json.dumps({}).encode()
    config_digest, config_size = upload_blob(base, args.repo, config, auth)
    layer_digest, layer_size = upload_blob(base, args.repo, layer, auth)

    manifest = json.dumps(
        {
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.module.wasm.config.v1+json",
                "digest": config_digest,
                "size": config_size,
            },
            "layers": [
                {"mediaType": layer_media, "digest": layer_digest, "size": layer_size}
            ],
        },
        separators=(",", ":"),
    ).encode()
    status, _, body = request(
        "PUT",
        f"{base}/v2/{args.repo}/manifests/{args.tag}",
        data=manifest,
        headers={"Content-Type": "application/vnd.oci.image.manifest.v1+json"},
        auth=auth,
    )
    assert status == 201, f"manifest PUT failed: {status} {body!r}"
    print(f"pushed: {args.registry}/{args.repo}:{args.tag}")
    print(f"pinned: {args.registry}/{args.repo}@sha256:{sha256_hex(manifest)}")


if __name__ == "__main__":
    main()
