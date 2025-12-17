#!/bin/bash
# TCP 客户端测试脚本

PORT=${1:-19000}
HOST=${2:-127.0.0.1}

echo "=== Testing TCP connection to $HOST:$PORT ==="
echo "Sending test message..."
echo -e "GET / HTTP/1.1\r\nHost: test.example.com\r\n\r\n" | nc -v $HOST $PORT

echo ""
echo "=== Test completed ==="
