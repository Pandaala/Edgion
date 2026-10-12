#!/usr/bin/env bash
# Build current checkout in a native Rocky 9 container and export only product bins.
set -euo pipefail
script_dir=$(cd "$(dirname "$0")" && pwd)
repo_root=$(cd "$script_dir/../../../../.." && pwd)
arch=arm64
output=
builder_image=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --arch) arch=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        --image) builder_image=$2; shift 2 ;;
        --help) echo 'Usage: build-binaries.sh --arch arm64|amd64 --output DIR [--image IMAGE]'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$arch" in arm64|amd64) ;; *) echo 'Architecture must be arm64 or amd64' >&2; exit 2 ;; esac
[ -n "$output" ] || { echo '--output is required' >&2; exit 2; }
mkdir -p "$output"
output=$(cd "$output" && pwd)
if [ -z "$builder_image" ]; then
    builder_image="edgion-packaging-builder:rocky9-$arch"
    docker build --platform "linux/$arch" -t "$builder_image" \
        -f "$script_dir/../Dockerfile.builder" "$repo_root"
fi
docker run --rm --platform "linux/$arch" \
    --mount "type=bind,source=$repo_root,target=/source,readonly" \
    --mount "type=bind,source=$output,target=/artifacts" \
    --mount "type=volume,source=edgion-packaging-cargo-$arch,target=/root/.cargo/registry" \
    --mount "type=volume,source=edgion-packaging-git-$arch,target=/root/.cargo/git" \
    --mount "type=volume,source=edgion-packaging-target-$arch,target=/build-target" \
    -e CARGO_TARGET_DIR=/build-target "$builder_image" bash -euc '
        cargo build --locked --profile production \
            -p edgion-controller -p edgion-gateway -p edgion-cli \
            --bin edgion-controller --bin edgion-gateway --bin edgion-cli
        for name in edgion-controller edgion-gateway edgion-cli; do
            install -m 0755 "/build-target/production/$name" "/artifacts/$name"
        done
        sha256sum /source/Cargo.lock > /artifacts/cargo-lock.sha256
        rustc --version > /artifacts/rust-version.txt
        rpm -q glibc > /artifacts/build-libc.txt
    '

mkdir -p "$output/waf/licenses"
cp "$repo_root/edgion-waf/INJECTION.md" "$output/waf/INJECTION.md"
cp "$repo_root/edgion-waf/licenses/"*.txt "$output/waf/licenses/"
python3 "$repo_root/cicd/checks/check_waf_distribution_notices.py" --directory "$output" --prefix waf
