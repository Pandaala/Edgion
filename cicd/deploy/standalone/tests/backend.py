"""An isolated HTTP backend with a deterministic in-flight-request handshake."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import time


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/slow":
            Path("/run/package-slow-started").touch()
            time.sleep(3)
        body = b"edgion-package-backend\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


ThreadingHTTPServer(("0.0.0.0", 18080), Handler).serve_forever()
