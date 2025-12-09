#!/bin/bash
# Edgion 开发测试启动脚本
# 同时启动 edgion-op 和 edgion-gw，日志分别输出

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 项目根目录 (脚本在 tests 目录下)
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# 切换到项目根目录，让 cargo 能找到 Cargo.toml
cd "$PROJECT_DIR"

# 日志目录 (在 tests 目录下)
LOG_DIR="${SCRIPT_DIR}/logs"
mkdir -p "$LOG_DIR"

# 运行时目录 (用于存放 prefix 数据)
RUNTIME_DIR="${SCRIPT_DIR}/runtime"
mkdir -p "$RUNTIME_DIR"

# 日志文件
EDGION_GW_LOG="${LOG_DIR}/edgion_gw.log"
EDGION_OP_LOG="${LOG_DIR}/edgion_op.log"
ACCESS_LOG="${LOG_DIR}/access.log"

# PID 文件
PID_DIR="${LOG_DIR}/pids"
mkdir -p "$PID_DIR"

echo_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

echo_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

echo_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

echo_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 清理函数
cleanup() {
    echo ""
    echo_info "正在停止所有服务..."
    
    # 停止所有后台进程
    if [ -f "${PID_DIR}/edgion_gw.pid" ]; then
        kill $(cat "${PID_DIR}/edgion_gw.pid") 2>/dev/null && echo_success "edgion-gw 已停止" || true
        rm -f "${PID_DIR}/edgion_gw.pid"
    fi
    
    if [ -f "${PID_DIR}/edgion_op.pid" ]; then
        kill $(cat "${PID_DIR}/edgion_op.pid") 2>/dev/null && echo_success "edgion-op 已停止" || true
        rm -f "${PID_DIR}/edgion_op.pid"
    fi
    
    echo_success "清理完成"
    exit 0
}

# 捕获 Ctrl+C
trap cleanup SIGINT SIGTERM

# 显示帮助
show_help() {
    echo "用法: $0 [选项]"
    echo ""
    echo "选项:"
    echo "  start     启动所有服务 (默认)"
    echo "  stop      停止所有服务"
    echo "  restart   重启所有服务"
    echo "  status    查看服务状态"
    echo "  logs      实时查看所有日志"
    echo "  clean     清理日志文件"
    echo "  help      显示此帮助"
    echo ""
    echo "日志位置: ${LOG_DIR}"
    echo "  - edgion_op.log    Operator日志"
    echo "  - edgion_gw.log    网关日志"
    echo "  - access.log       访问日志"
}

# 启动 edgion-gw
start_edgion_gw() {
    echo_info "启动 edgion-gw..."
    
    if [ -f "${PID_DIR}/edgion_gw.pid" ] && kill -0 $(cat "${PID_DIR}/edgion_gw.pid") 2>/dev/null; then
        echo_warn "edgion-gw 已在运行 (PID: $(cat ${PID_DIR}/edgion_gw.pid))"
        return
    fi
    
    # 打印启动命令
    echo_info "执行命令 (工作目录: $PROJECT_DIR):"
    echo "  EDGION_ACCESS_LOG=$ACCESS_LOG RUST_LOG=debug cargo run -p edgion --bin edgion-gw -- -p $RUNTIME_DIR --gateway-class public-gateway --server-addr http://127.0.0.1:50061"
    
    # 设置环境变量让网关输出访问日志到指定文件
    EDGION_ACCESS_LOG="$ACCESS_LOG" \
    RUST_LOG=debug \
    cargo run -p edgion --bin edgion-gw -- \
        -p "$RUNTIME_DIR" \
        --gateway-class public-gateway \
        --server-addr http://127.0.0.1:50061 \
        > "$EDGION_GW_LOG" 2>&1 &
    echo $! > "${PID_DIR}/edgion_gw.pid"
    sleep 2
    
    if kill -0 $(cat "${PID_DIR}/edgion_gw.pid") 2>/dev/null; then
        echo_success "edgion-gw 已启动 (PID: $(cat ${PID_DIR}/edgion_gw.pid))"
        echo "         日志: $EDGION_GW_LOG"
        echo "         访问日志: $ACCESS_LOG"
    else
        echo_error "edgion-gw 启动失败，请查看日志: $EDGION_GW_LOG"
    fi
}

# 启动 edgion-op
start_edgion_op() {
    echo_info "启动 edgion-op..."
    
    if [ -f "${PID_DIR}/edgion_op.pid" ] && kill -0 $(cat "${PID_DIR}/edgion_op.pid") 2>/dev/null; then
        echo_warn "edgion-op 已在运行 (PID: $(cat ${PID_DIR}/edgion_op.pid))"
        return
    fi
    
    # 打印启动命令
    echo_info "执行命令 (工作目录: $PROJECT_DIR):"
    echo "  RUST_LOG=debug cargo run -p edgion --bin edgion-op -- -p $RUNTIME_DIR --gateway-class public-gateway --grpc-listen 127.0.0.1:50061 --loader-type local_path --loader-dir ${PROJECT_DIR}/config/examples"
    
    RUST_LOG=debug \
    cargo run -p edgion --bin edgion-op -- \
        -p "$RUNTIME_DIR" \
        --gateway-class public-gateway \
        --grpc-listen 127.0.0.1:50061 \
        --loader-type local_path \
        --loader-dir "${PROJECT_DIR}/config/examples" \
        > "$EDGION_OP_LOG" 2>&1 &
    echo $! > "${PID_DIR}/edgion_op.pid"
    sleep 3
    
    if kill -0 $(cat "${PID_DIR}/edgion_op.pid") 2>/dev/null; then
        echo_success "edgion-op 已启动 (PID: $(cat ${PID_DIR}/edgion_op.pid))"
        echo "         日志: $EDGION_OP_LOG"
    else
        echo_error "edgion-op 启动失败，请查看日志: $EDGION_OP_LOG"
    fi
}

# 启动所有服务
start_all() {
    echo ""
    echo "=========================================="
    echo "       Edgion 开发环境启动"
    echo "=========================================="
    echo ""
    
    # 清空旧日志
    > "$EDGION_OP_LOG"
    > "$EDGION_GW_LOG"
    > "$ACCESS_LOG"
    
    start_edgion_op
    start_edgion_gw
    
    echo ""
    echo "=========================================="
    echo_success "所有服务已启动!"
    echo "=========================================="
    echo ""
    echo "日志目录: ${LOG_DIR}"
    echo "  tail -f ${LOG_DIR}/edgion_op.log"
    echo "  tail -f ${LOG_DIR}/edgion_gw.log"
    echo "  tail -f ${LOG_DIR}/access.log"
    echo ""
    echo "测试命令:"
    echo "  curl -k -H 'Host: aaa.example.com' https://127.0.0.1:18443/aaa/doc/"
    echo ""
    echo "按 Ctrl+C 停止所有服务"
    echo ""
    
    # 等待用户中断
    wait
}

# 停止所有服务
stop_all() {
    echo_info "停止所有服务..."
    cleanup
}

# 查看状态
show_status() {
    echo ""
    echo "服务状态:"
    echo "---------"
    
    if [ -f "${PID_DIR}/edgion_op.pid" ] && kill -0 $(cat "${PID_DIR}/edgion_op.pid") 2>/dev/null; then
        echo_success "edgion-op    运行中 (PID: $(cat ${PID_DIR}/edgion_op.pid))"
    else
        echo_warn "edgion-op    未运行"
    fi
    
    if [ -f "${PID_DIR}/edgion_gw.pid" ] && kill -0 $(cat "${PID_DIR}/edgion_gw.pid") 2>/dev/null; then
        echo_success "edgion-gw    运行中 (PID: $(cat ${PID_DIR}/edgion_gw.pid))"
    else
        echo_warn "edgion-gw    未运行"
    fi
    echo ""
}

# 实时查看日志
show_logs() {
    echo_info "实时查看所有日志 (Ctrl+C 退出)..."
    tail -f "$EDGION_OP_LOG" "$EDGION_GW_LOG" "$ACCESS_LOG" 2>/dev/null
}

# 清理日志
clean_logs() {
    echo_info "清理日志文件..."
    rm -f "${LOG_DIR}"/*.log
    echo_success "日志已清理"
}

# 主入口
case "${1:-start}" in
    start)
        start_all
        ;;
    stop)
        stop_all
        ;;
    restart)
        stop_all
        sleep 2
        start_all
        ;;
    status)
        show_status
        ;;
    logs)
        show_logs
        ;;
    clean)
        clean_logs
        ;;
    help|--help|-h)
        show_help
        ;;
    *)
        echo_error "未知命令: $1"
        show_help
        exit 1
        ;;
esac

