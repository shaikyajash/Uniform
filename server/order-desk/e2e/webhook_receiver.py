"""A stand-in for a shop's server: takes Uniform's webhooks and checks their signature.

Usage: webhook_receiver.py <port> <state dir>
Reads the signing secret from <state dir>/webhook.secret on every request (shop.sh writes it),
and appends one JSON line per webhook to <state dir>/webhooks.jsonl with "valid": true or false.
Replies 200 to a valid signature and 401 to anything else, as a real shop should.
"""

import hashlib
import hmac
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

PORT = int(sys.argv[1])
STATE = Path(sys.argv[2])


class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", "0")))
        secret_file = STATE / "webhook.secret"
        secret = secret_file.read_text().strip() if secret_file.exists() else ""
        expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
        valid = bool(secret) and hmac.compare_digest(expected, self.headers.get("x-uniform-signature", ""))
        line = {"event": self.headers.get("x-uniform-event"), "valid": valid, "body": json.loads(body or b"null")}
        with open(STATE / "webhooks.jsonl", "a") as log:
            log.write(json.dumps(line) + "\n")
        self.send_response(200 if valid else 401)
        self.end_headers()

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", PORT), Receiver).serve_forever()
