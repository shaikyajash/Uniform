"""Stand-in for the Order Desk during the local end-to-end test.

GET /v1/orders/<id>    -> the order from orders.json (shop address, amount, status)
POST /internal/events  -> appended to events.log, 200
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".state")
TOKEN = os.environ.get("INTERNAL_EVENTS_TOKEN", "test-token")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if not self.path.startswith("/v1/orders/"):
            return self.send(404, {"error": "not_found"})
        order_id = self.path.rsplit("/", 1)[1].lower()
        with open(os.path.join(HERE, "orders.json")) as f:
            orders = json.load(f)
        order = orders.get(order_id)
        if order is None:
            return self.send(404, {"error": "not_found"})
        self.send(200, {"orderId": order_id, **order})

    def do_POST(self):
        if self.path != "/internal/events":
            return self.send(404, {"error": "not_found"})
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            return self.send(401, {"error": "unauthorized"})
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        with open(os.path.join(HERE, "events.log"), "a") as f:
            f.write(body.decode() + "\n")
        self.send(200, {"stored": True})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8603
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
