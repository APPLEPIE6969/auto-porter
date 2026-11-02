#!/usr/bin/env python3
"""Mock LLM endpoint for retry-policy tests.

Modes:
  flaky N   -> 429 (Retry-After: 0) for the first N requests, then 200 with a
               minimal OpenAI-style completion; counts requests in counter file
  reject    -> 401 always (auth fatal fast-fail)
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE = sys.argv[1]
FLAKY_N = int(sys.argv[2]) if MODE == "flaky" else 0
COUNTER = sys.argv[3]
PORT = int(sys.argv[4])


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        count = 0
        try:
            count = int(open(COUNTER).read())
        except Exception:
            pass
        count += 1
        open(COUNTER, "w").write(str(count))

        self.rfile.read(int(self.headers.get("content-length", 0)))
        auth = self.headers.get("authorization", "")

        if not auth.startswith("Bearer "):
            body = b'{"error":{"message":"Missing Authentication header","code":401}}'
            self.send_response(401)
        elif MODE == "reject":
            body = b'{"error":{"message":"Invalid key","code":401}}'
            self.send_response(401)
        elif MODE == "flaky" and count <= FLAKY_N:
            self.send_response(429)
            self.send_header("Retry-After", "0")
            body = b'{"error":{"message":"Rate limit exceeded","code":429}}'
        else:
            body = json.dumps({
                "choices": [{"message": {"content":
                    "/// Generated hook implementation.\nfn noop() {}"
                }}]
            }).encode()
            self.send_response(200)

        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
