#!/usr/bin/env bash
# Center 无认证配置启动的集成测试。
#
# 验证 Center 在 TOML/YAML 既无 [local_auth] 也无 [auth] 时：
# 1. 启动成功（不 panic、不 exit）
# 2. /health 返回 200
# 3. /api/v1/auth/status 返回 200
# 4. /api/v1/controllers (或任意业务路径) 无 token 返回 503
#    (middleware 的 fail-close 分支——require_auth=true + no provider ready → 503)
# 5. 启动日志中有 WARN "No authentication configured"
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

TMPDIR=$(mktemp -d)

# 极简 Center 配置，无 auth / local_auth 段
cat > "$TMPDIR/center.yaml" <<EOF
server:
  http_addr: "127.0.0.1:58100"
  grpc_addr: "127.0.0.1:58110"
database:
  enabled: false
sync:
  command_timeout_secs: 30
grpc_security:
  skip_tls: true
EOF

cd "$REPO_ROOT"

# 端口预检：避免上一次遗留进程或并行 CI 占用导致启动失败时报错不清晰。
for port in 58100 58110; do
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "FAIL: port $port already in use; stop the conflicting process or rerun later"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        exit 1
    fi
done

# 确保 edgion-center 已构建
if [ ! -x "$REPO_ROOT/target/debug/edgion-center" ]; then
    echo "Building edgion-center..."
    cargo build --bin edgion-center
fi

"$REPO_ROOT/target/debug/edgion-center" -c "$TMPDIR/center.yaml" > "$TMPDIR/center.log" 2>&1 &
CENTER_PID=$!

cleanup() {
    kill "$CENTER_PID" 2>/dev/null || true
    wait "$CENTER_PID" 2>/dev/null || true
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

# 等 HTTP 端口就绪
READY=false
for _ in $(seq 1 20); do
    if curl -sf http://127.0.0.1:58100/health > /dev/null 2>&1; then
        READY=true
        break
    fi
    if ! kill -0 "$CENTER_PID" 2>/dev/null; then
        echo "FAIL: center exited during startup"
        cat "$TMPDIR/center.log"
        exit 1
    fi
    sleep 1
done

if ! $READY; then
    echo "FAIL: center HTTP /health did not become ready within 20s"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[1/4] Testing /health..."
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58100/health)
if [ "$STATUS" != "200" ]; then
    echo "FAIL: /health returned $STATUS (expected 200)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[2/4] Testing /api/v1/auth/status..."
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58100/api/v1/auth/status)
if [ "$STATUS" != "200" ]; then
    echo "FAIL: /auth/status returned $STATUS (expected 200)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[3/4] Testing business route returns 503 (fail-close via 503)..."
# Center 的业务路径：/api/v1/controllers 无 token 应该 503（not 200 not 401）
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58100/api/v1/controllers)
if [ "$STATUS" != "503" ]; then
    echo "FAIL: business route /api/v1/controllers returned $STATUS (expected 503 fail-close)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[4/4] Checking startup WARN log..."
if ! grep -q "No authentication configured" "$TMPDIR/center.log"; then
    echo "FAIL: Expected WARN log 'No authentication configured' not found"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "PASS: Center no-auth startup scenario"
