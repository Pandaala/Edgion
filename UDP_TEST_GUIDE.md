# UDP Proxy Testing Guide

This guide demonstrates how to test the UDP proxy functionality of Edgion Gateway.

## Architecture

```
Client (nc) --UDP--> Edgion Gateway (port 19001) --UDP--> Backend Service (port 5353)
```

## Prerequisites

1. Edgion Gateway is running with UDP listener configured
2. Backend UDP service is available
3. `nc` (netcat) command is available for testing

## Configuration Files

### 1. Gateway Configuration

File: `config/examples/Gateway_default_gateway1.yaml`

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gateway1
  namespace: default
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: udp19001
      protocol: UDP
      port: 19001
```

### 2. UDPRoute Configuration

File: `config/examples/UDPRoute_default_example-udp-route.yaml`

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: example-udp-route
  namespace: default
spec:
  parentRefs:
    - name: gateway1
      namespace: default
      sectionName: udp19001
      port: 19001  # Must match Gateway listener port
  rules:
    - backendRefs:
        - name: udp-service
          namespace: default
          port: 5353
          weight: 1
```

### 3. EndpointSlice Configuration

File: `config/examples/EndpointSlice_default_udp-service-xxxxx.yaml`

```yaml
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: udp-service-xxxxx
  namespace: default
  labels:
    kubernetes.io/service-name: udp-service
addressType: IPv4
ports:
  - name: dns
    port: 5353
    protocol: UDP
endpoints:
  - addresses:
      - "127.0.0.1"  # Backend service IP
    conditions:
      ready: true
```

## Testing Steps

### Step 1: Start Backend UDP Service

Create a simple UDP echo server for testing:

```bash
# Start a UDP echo server on port 5353
nc -ul 5353
```

Or use Python:

```python
# udp_echo_server.py
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(('0.0.0.0', 5353))
print("UDP echo server listening on port 5353")

while True:
    data, addr = sock.recvfrom(1024)
    print(f"Received from {addr}: {data.decode()}")
    sock.sendto(data, addr)  # Echo back
```

```bash
python3 udp_echo_server.py
```

### Step 2: Start Edgion Gateway

```bash
cd /Users/caohao/code/Edgion

# Compile the project (if not already compiled)
cargo build --release

# Start the gateway
./target/release/edgion-gateway --config-file ./config/edgion-gateway.toml
```

### Step 3: Verify Gateway Logs

You should see UDP listener in the logs:

```
INFO edgion::core::gateway::gateway_base: Listening on gateway=default/gateway1 listener=udp19001 addr=0.0.0.0:19001 protocol=UDP
```

### Step 4: Test UDP Proxy

#### Test with netcat:

```bash
# Send a UDP packet to the gateway
echo "Hello UDP" | nc -u 127.0.0.1 19001

# For interactive testing (type messages and press Enter)
nc -u 127.0.0.1 19001
```

#### Test with Python client:

```python
# udp_client.py
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(5.0)

# Send message to gateway
message = b"Hello UDP Gateway!"
sock.sendto(message, ('127.0.0.1', 19001))
print(f"Sent: {message.decode()}")

# Receive response
try:
    data, addr = sock.recvfrom(1024)
    print(f"Received: {data.decode()} from {addr}")
except socket.timeout:
    print("No response received (timeout)")

sock.close()
```

```bash
python3 udp_client.py
```

### Step 5: Verify Traffic Flow

1. **Gateway logs**: Should show minimal or no logs (UDP proxy is designed for performance)
2. **Backend service**: Should receive the UDP packets
3. **Client**: Should receive responses if backend echoes back

## Load Balancing Test

To test load balancing with multiple backends:

1. Start multiple backend UDP services on different ports (e.g., 5353, 5354, 5355)
2. Create EndpointSlice with multiple endpoints
3. Send multiple UDP packets from client
4. Verify that packets are distributed across backends (Round Robin by default)

Example EndpointSlice with multiple backends:

```yaml
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: udp-service-multi
  namespace: default
  labels:
    kubernetes.io/service-name: udp-service
addressType: IPv4
ports:
  - name: dns
    port: 5353
    protocol: UDP
endpoints:
  - addresses:
      - "127.0.0.1"
    conditions:
      ready: true
  - addresses:
      - "127.0.0.2"
    conditions:
      ready: true
  - addresses:
      - "127.0.0.3"
    conditions:
      ready: true
```

## Session Affinity

The UDP proxy maintains client sessions for 60 seconds. During this time:
- Packets from the same client IP:port go to the same backend
- Each client gets its own dedicated upstream socket
- Responses from backend are forwarded back to the correct client

## Performance Considerations

1. **No Access Logs**: UDP proxy doesn't generate access logs to maximize performance
2. **Asynchronous Processing**: Each UDP packet is processed in a separate tokio task
3. **Session Cleanup**: Inactive sessions are automatically cleaned up after 60 seconds
4. **Memory Efficient**: Uses lock-free DashMap for concurrent session management

## Troubleshooting

### Issue: No UDP listener in logs

**Solution**: Check Gateway configuration has `protocol: UDP` and port is specified

### Issue: Packets not reaching backend

**Solution**: 
1. Verify UDPRoute has correct `port` in `parentRefs`
2. Check EndpointSlice has `kubernetes.io/service-name` label matching backend service
3. Ensure backend endpoints have `ready: true` condition

### Issue: No response from backend

**Solution**:
1. Verify backend service actually sends responses
2. Check firewall rules allow UDP traffic
3. Test backend directly (bypass gateway) to ensure it works

### Issue: Session timeout too short/long

**Solution**: Modify `SESSION_TIMEOUT` constant in `edgion_udp.rs` (default: 60 seconds)

## Differences from TCP Proxy

| Feature | TCP | UDP |
|---------|-----|-----|
| Connection | Connection-oriented | Connectionless |
| Access Logs | Yes (on connection close) | No (performance) |
| Session Management | Automatic | Manual (60s timeout) |
| Reliability | Guaranteed delivery | Best effort |
| Order | Preserved | Not guaranteed |
| Use Cases | HTTP, gRPC, databases | DNS, gaming, streaming |

## Example Use Cases

1. **DNS Proxy**: Forward DNS queries (port 53)
2. **Gaming**: Low-latency game server proxy
3. **IoT**: Sensor data collection
4. **VoIP**: Voice/video streaming
5. **Service Discovery**: Consul, etcd UDP endpoints

## Advanced Configuration

### Custom Session Timeout

Edit `src/core/routes/udp_routes/edgion_udp.rs`:

```rust
const SESSION_TIMEOUT: Duration = Duration::from_secs(120); // 2 minutes
```

### Maximum Packet Size

Default: 65535 bytes (maximum UDP packet size)

To change:

```rust
const MAX_UDP_PACKET_SIZE: usize = 8192; // 8KB
```

## Cleanup

After testing, stop the services:

```bash
# Stop gateway
pkill -9 edgion-gateway

# Stop backend service
pkill -9 nc  # or pkill -9 python3
```

