"""A range server, throttled per connection, for indir's GUI run: serve.py <dir> <port> <bytes/s>."""
import os
import sys
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT, PORT, RATE = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=ROOT, **k)

    def log_message(self, *a):
        pass

    def do_GET(self):
        path = os.path.join(ROOT, self.path.lstrip("/"))
        if not os.path.isfile(path):
            self.send_error(404)
            return
        size = os.path.getsize(path)
        start, end = 0, size - 1
        rng = self.headers.get("Range")
        if rng and rng.startswith("bytes="):
            a, _, b = rng[6:].partition("-")
            start = int(a) if a else 0
            end = int(b) if b else size - 1
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        else:
            self.send_response(200)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("ETag", '"fixed"')
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(start)
            left = end - start + 1
            chunk = max(1, RATE // 20)
            try:
                while left > 0:
                    data = f.read(min(chunk, left))
                    self.wfile.write(data)
                    left -= len(data)
                    time.sleep(0.05)
            except (BrokenPipeError, ConnectionResetError):
                pass


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
