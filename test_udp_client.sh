#!/bin/bash

# Simple UDP client test script for Edgion Gateway
# Usage: ./test_udp_client.sh [gateway_host] [gateway_port]

GATEWAY_HOST=${1:-"127.0.0.1"}
GATEWAY_PORT=${2:-"19001"}

echo "=== Testing UDP connection to ${GATEWAY_HOST}:${GATEWAY_PORT} ==="
echo ""

# Test 1: Single message
echo "Test 1: Sending single UDP message..."
echo "Hello from UDP client" | nc -u -w1 ${GATEWAY_HOST} ${GATEWAY_PORT}
echo "Message sent"
echo ""

# Test 2: Multiple messages
echo "Test 2: Sending multiple UDP messages..."
for i in {1..5}; do
    echo "UDP message $i" | nc -u -w1 ${GATEWAY_HOST} ${GATEWAY_PORT}
    echo "Sent message $i"
    sleep 0.5
done
echo ""

echo "=== Test completed ==="
echo ""
echo "If backend service is an echo server, you should see responses above."
echo "Check gateway logs for any errors."

