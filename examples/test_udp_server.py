#!/usr/bin/env python3
"""
Simple UDP echo server for testing Edgion Gateway UDP proxy functionality.

This server listens on a specified port and echoes back any received UDP packets.
"""

import socket
import sys
from datetime import datetime

def main():
    # Default port
    port = 5353
    
    # Override port from command line if provided
    if len(sys.argv) > 1:
        try:
            port = int(sys.argv[1])
        except ValueError:
            print(f"Invalid port number: {sys.argv[1]}")
            sys.exit(1)
    
    # Create UDP socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    
    # Bind to all interfaces
    server_address = ('0.0.0.0', port)
    sock.bind(server_address)
    
    print(f"=== UDP Echo Server Started ===")
    print(f"Listening on: {server_address[0]}:{server_address[1]}")
    print(f"Press Ctrl+C to stop")
    print("=" * 32)
    print()
    
    packet_count = 0
    
    try:
        while True:
            # Receive data
            data, client_address = sock.recvfrom(4096)
            packet_count += 1
            
            timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
            
            # Decode and print
            try:
                message = data.decode('utf-8')
                print(f"[{timestamp}] Packet #{packet_count}")
                print(f"  From: {client_address[0]}:{client_address[1]}")
                print(f"  Size: {len(data)} bytes")
                print(f"  Message: {message}")
            except UnicodeDecodeError:
                print(f"[{timestamp}] Packet #{packet_count}")
                print(f"  From: {client_address[0]}:{client_address[1]}")
                print(f"  Size: {len(data)} bytes")
                print(f"  Data (hex): {data.hex()}")
            
            # Echo back
            sock.sendto(data, client_address)
            print(f"  Echoed back to client")
            print()
            
    except KeyboardInterrupt:
        print("\n\n=== Server Stopped ===")
        print(f"Total packets received: {packet_count}")
    finally:
        sock.close()

if __name__ == '__main__':
    main()

