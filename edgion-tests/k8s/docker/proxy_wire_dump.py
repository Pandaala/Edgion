#!/usr/bin/env python3

import http.server
import socket
import threading


CAPTURES = {"v1": "", "v2": ""}
CAPTURE_LOCK = threading.Lock()


def capture_loop(listener, version):
    while True:
        connection, _ = listener.accept()
        try:
            connection.settimeout(5)
            payload = connection.recv(64).hex()
            with CAPTURE_LOCK:
                CAPTURES[version] = payload
        finally:
            connection.close()


class CaptureHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        version = self.path.lstrip("/")
        if version not in CAPTURES:
            self.send_response(404)
            self.end_headers()
            return
        with CAPTURE_LOCK:
            body = CAPTURES[version].encode("ascii")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        return


def make_listener(port):
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", port))
    listener.listen(8)
    return listener


def main():
    listeners = ((make_listener(58444), "v2"), (make_listener(58445), "v1"))
    for listener, version in listeners:
        threading.Thread(target=capture_loop, args=(listener, version), daemon=True).start()
    http.server.ThreadingHTTPServer(("0.0.0.0", 18080), CaptureHandler).serve_forever()


if __name__ == "__main__":
    main()
