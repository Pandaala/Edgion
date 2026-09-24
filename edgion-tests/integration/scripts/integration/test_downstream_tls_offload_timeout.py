#!/usr/bin/env python3
"""Verify that timed-out downstream TLS handshakes release their sockets."""

import selectors
import socket
import time


HOST = "127.0.0.1"
PORT = 31190
CONNECTIONS = 4
MIN_TIMEOUT_SECONDS = 50
MAX_TIMEOUT_SECONDS = 75


def main() -> None:
    sockets = []
    selector = selectors.DefaultSelector()
    try:
        for _ in range(CONNECTIONS):
            connection = socket.create_connection((HOST, PORT), timeout=5)
            connection.setblocking(False)
            sockets.append(connection)
            selector.register(connection, selectors.EVENT_READ)

        started = time.monotonic()
        pending = set(sockets)
        while pending and time.monotonic() - started < MAX_TIMEOUT_SECONDS:
            for key, _ in selector.select(timeout=1):
                connection = key.fileobj
                try:
                    data = connection.recv(1)
                except ConnectionResetError:
                    data = b""
                if data:
                    raise AssertionError("idle TLS connection received unexpected data")
                elapsed = time.monotonic() - started
                if elapsed < MIN_TIMEOUT_SECONDS:
                    raise AssertionError(f"TLS connection closed before handshake timeout: {elapsed:.1f}s")
                selector.unregister(connection)
                pending.remove(connection)

        if pending:
            raise AssertionError(f"{len(pending)} TLS connections remained open after {MAX_TIMEOUT_SECONDS}s")
        print(f"All {CONNECTIONS} incomplete TLS handshakes closed after the listener timeout")
    finally:
        selector.close()
        for connection in sockets:
            connection.close()


if __name__ == "__main__":
    main()
