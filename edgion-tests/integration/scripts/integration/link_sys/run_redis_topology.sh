#!/usr/bin/env bash
# Redis Cluster/Sentinel/standalone topology checks.
# These tests dial the fixture directly and do not start a Controller or Gateway.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../../" && pwd)"
COMPOSE=(docker compose -p edgion-redis-migration -f "$ROOT/edgion-tests/integration/conf/Services/redis/docker-compose.topologies.yaml")

cleanup() {
    "${COMPOSE[@]}" down --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

"${COMPOSE[@]}" up -d --force-recreate --wait
cd "$ROOT"
cargo test -p edgion-tests --test redis_migration -- --ignored --test-threads=1
