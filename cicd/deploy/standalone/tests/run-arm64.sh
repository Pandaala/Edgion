#!/usr/bin/env bash
# Run only disposable containers owned by this invocation. No host ports/sysctls.
set -euo pipefail
test_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
packages=
reports=
targets=debian12,ubuntu2404,rocky9
while (($#)); do
    case "$1" in
        --packages) packages=$2; shift 2 ;;
        --report-dir) reports=$2; shift 2 ;;
        --targets) targets=$2; shift 2 ;;
        *) echo "Usage: $0 --packages DIR --report-dir DIR [--targets debian12,ubuntu2404,rocky9]" >&2; exit 2 ;;
    esac
done
[[ -d "$packages" && -n "$reports" ]] || { echo "Package and report directories are required" >&2; exit 2; }
packages=$(CDPATH='' cd -- "$packages" && pwd)
mkdir -p "$reports"
reports=$(CDPATH='' cd -- "$reports" && pwd)
run_id="edgion-package-$(date +%s)-$$"
owned=()
cleanup() {
    for name in "${owned[@]}"; do docker rm -f "$name" >/dev/null 2>&1 || true; done
}
collect_package_error() {
    if docker exec "$name" test -f /var/lib/package-test/package-output >/dev/null 2>&1; then
        docker cp "$name:/var/lib/package-test/package-output" "$reports/$target-package-error.log" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
docker info --format '{{.OSType}} {{.Architecture}}' >"$reports/docker-engine.txt"
case "$(cat "$reports/docker-engine.txt")" in
    'linux aarch64'|'linux arm64') ;;
    *) echo "A native arm64 Linux Docker engine is required" >&2; exit 2 ;;
esac
IFS=, read -r -a selected <<<"$targets"
for target in "${selected[@]}"; do
    case "$target" in
        debian12) base=debian:bookworm-slim; format=deb ;;
        ubuntu2404) base=ubuntu:24.04; format=deb ;;
        rocky9) base=rockylinux/rockylinux:9; format=rpm ;;
        *) echo "Unsupported target: $target" >&2; exit 2 ;;
    esac
    name="$run_id-$target"
    image="edgion-package-test:$target-arm64"
    docker build --platform linux/arm64 --build-arg "BASE=$base" -t "$image" "$test_dir" >"$reports/$target-image.log" 2>&1
    owned+=("$name")
    docker run -d --name "$name" --platform linux/arm64 --privileged --cgroupns=private \
        --tmpfs /run --tmpfs /run/lock -e container=docker \
        --mount "type=bind,src=$packages,dst=/packages,readonly" \
        --mount "type=bind,src=$test_dir,dst=/tests,readonly" "$image" >/dev/null
    if ! docker exec "$name" bash /tests/lifecycle.sh "$format" prepare >"$reports/$target-lifecycle.log" 2>&1; then
        collect_package_error
        echo "FAIL: $target; see $reports/$target-lifecycle.log" >&2; exit 1
    fi
    docker restart --time 100 "$name" >/dev/null
    if ! docker exec "$name" bash /tests/lifecycle.sh "$format" finish >>"$reports/$target-lifecycle.log" 2>&1; then
        collect_package_error
        echo "FAIL: $target; see $reports/$target-lifecycle.log" >&2; exit 1
    fi
    docker cp "$name:/var/lib/package-test/report.json" "$reports/$target-report.json" >/dev/null
    docker rm -f "$name" >/dev/null
    echo "PASS: $target arm64 $format (real systemd, package revision upgrade/rollback)"
done
