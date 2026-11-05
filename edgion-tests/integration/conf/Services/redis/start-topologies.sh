#!/bin/sh
# Redis-only topology fixture; all nodes share one container network namespace.
# Announced loopback ports are identical inside the container and on the host.
set -eu
mkdir -p /tmp/redis-topologies
for port in 16400 16401 16402 16403 16404 16405; do
    mkdir -p "/tmp/redis-topologies/$port"
    redis-server --port "$port" --bind 0.0.0.0 --protected-mode no \
        --cluster-enabled yes --cluster-config-file nodes.conf \
        --cluster-node-timeout 1000 --cluster-announce-ip 127.0.0.1 \
        --cluster-announce-port "$port" --dir "/tmp/redis-topologies/$port" \
        --save '' --appendonly no --logfile "/tmp/redis-topologies/$port/server.log" --daemonize yes
 done
redis-server --port 16420 --bind 0.0.0.0 --protected-mode no --save '' --daemonize yes
redis-server --port 16421 --bind 0.0.0.0 --protected-mode no --save '' \
    --replicaof 127.0.0.1 16420 --daemonize yes
for port in 16410 16411 16412; do
    config="/tmp/redis-topologies/sentinel-$port.conf"
    cat > "$config" <<CONFIG
port $port
bind 0.0.0.0
protected-mode no
sentinel monitor edgion-primary 127.0.0.1 16420 2
sentinel down-after-milliseconds edgion-primary 1000
sentinel failover-timeout edgion-primary 10000
sentinel announce-ip 127.0.0.1
sentinel announce-port $port
CONFIG
    redis-server "$config" --sentinel --daemonize yes
 done
attempt=0
until redis-cli -p 16400 ping >/dev/null 2>&1 \
    && redis-cli -p 16401 ping >/dev/null 2>&1 \
    && redis-cli -p 16402 ping >/dev/null 2>&1 \
    && redis-cli -p 16403 ping >/dev/null 2>&1 \
    && redis-cli -p 16404 ping >/dev/null 2>&1 \
    && redis-cli -p 16405 ping >/dev/null 2>&1 \
    && redis-cli -p 16420 ping >/dev/null 2>&1 \
    && redis-cli -p 16410 ping >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 30 ]; then
        cat /tmp/redis-topologies/16400/server.log
        exit 1
    fi
    sleep 1
done
redis-cli --cluster create 127.0.0.1:16400 127.0.0.1:16401 \
    127.0.0.1:16402 127.0.0.1:16403 127.0.0.1:16404 127.0.0.1:16405 \
    --cluster-replicas 1 --cluster-yes
# Keep the fixture alive without another gateway/controller process.
exec tail -f /dev/null
