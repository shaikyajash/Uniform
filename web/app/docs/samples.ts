// The docs' code samples, one per language, all against the real API: `POST /v1/orders` with a
// Bearer API key, `GET /v1/orders/{id}`, and webhooks signed `hex(HMAC_SHA256(body, secret))` in
// `X-Uniform-Signature`. `base` is this site's API address, e.g. "https://uniform.app/desk/v1".

export type Sample = { lang: string; label: string; code: string };

/** The languages every server-side sample comes in, in tab order. */
export const LANGS = ["curl", "node", "python", "go", "php", "ruby", "java", "rust"] as const;
export type Lang = (typeof LANGS)[number];

export const LANG_LABEL: Record<Lang, string> = {
  curl: "cURL",
  node: "Node.js",
  python: "Python",
  go: "Go",
  php: "PHP",
  ruby: "Ruby",
  java: "Java",
  rust: "Rust",
};

/** Shiki's grammar for each tab. */
export const LANG_GRAMMAR: Record<Lang, string> = {
  curl: "bash",
  node: "javascript",
  python: "python",
  go: "go",
  php: "php",
  ruby: "ruby",
  java: "java",
  rust: "rust",
};

export function createOrder(base: string): Record<Lang, string> {
  return {
    curl: `curl ${base}/orders \\
  -H "Authorization: Bearer $UNIFORM_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "amount": "12.50",
    "reference": "order-1042",
    "successUrl": "https://yourshop.com/thanks"
  }'`,
    node: `const response = await fetch("${base}/orders", {
  method: "POST",
  headers: {
    Authorization: \`Bearer \${process.env.UNIFORM_API_KEY}\`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    amount: "12.50",
    reference: "order-1042",
    successUrl: "https://yourshop.com/thanks",
  }),
});

if (!response.ok) {
  const { error } = await response.json();
  throw new Error(\`\${error.code}: \${error.message}\`);
}
const { orderId, payUrl } = await response.json();`,
    python: `import os
import requests

response = requests.post(
    "${base}/orders",
    headers={"Authorization": f"Bearer {os.environ['UNIFORM_API_KEY']}"},
    json={
        "amount": "12.50",
        "reference": "order-1042",
        "successUrl": "https://yourshop.com/thanks",
    },
    timeout=10,
)
if not response.ok:
    error = response.json()["error"]
    raise RuntimeError(f"{error['code']}: {error['message']}")

order = response.json()
order_id, pay_url = order["orderId"], order["payUrl"]`,
    go: `package uniform

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
)

type Order struct {
	OrderID   string \`json:"orderId"\`
	Status    string \`json:"status"\`
	Amount    string \`json:"amount"\`
	PayURL    string \`json:"payUrl"\`
	ExpiresAt int64  \`json:"expiresAt"\`
}

func CreateOrder(amount, reference string) (*Order, error) {
	body, _ := json.Marshal(map[string]string{"amount": amount, "reference": reference})
	req, _ := http.NewRequest("POST", "${base}/orders", bytes.NewReader(body))
	req.Header.Set("Authorization", "Bearer "+os.Getenv("UNIFORM_API_KEY"))
	req.Header.Set("Content-Type", "application/json")

	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusCreated {
		return nil, fmt.Errorf("uniform: HTTP %d", res.StatusCode)
	}
	var order Order
	return &order, json.NewDecoder(res.Body).Decode(&order)
}`,
    php: `<?php
$ch = curl_init("${base}/orders");
curl_setopt_array($ch, [
    CURLOPT_POST => true,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_HTTPHEADER => [
        "Authorization: Bearer " . getenv("UNIFORM_API_KEY"),
        "Content-Type: application/json",
    ],
    CURLOPT_POSTFIELDS => json_encode([
        "amount" => "12.50",
        "reference" => "order-1042",
        "successUrl" => "https://yourshop.com/thanks",
    ]),
]);
$order = json_decode(curl_exec($ch), true);
if (curl_getinfo($ch, CURLINFO_HTTP_CODE) !== 201) {
    throw new RuntimeException($order["error"]["message"]);
}
$payUrl = $order["payUrl"];`,
    ruby: `require "net/http"
require "json"

uri = URI("${base}/orders")
request = Net::HTTP::Post.new(uri)
request["Authorization"] = "Bearer #{ENV.fetch('UNIFORM_API_KEY')}"
request["Content-Type"] = "application/json"
request.body = {
  amount: "12.50",
  reference: "order-1042",
  successUrl: "https://yourshop.com/thanks"
}.to_json

response = Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https") do |http|
  http.request(request)
end
order = JSON.parse(response.body)
raise order.dig("error", "message") unless response.code == "201"

pay_url = order["payUrl"]`,
    java: `import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

var body = """
    {"amount": "12.50", "reference": "order-1042", "successUrl": "https://yourshop.com/thanks"}
    """;
var request = HttpRequest.newBuilder(URI.create("${base}/orders"))
    .header("Authorization", "Bearer " + System.getenv("UNIFORM_API_KEY"))
    .header("Content-Type", "application/json")
    .POST(HttpRequest.BodyPublishers.ofString(body))
    .build();

var response = HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
if (response.statusCode() != 201) {
    throw new IllegalStateException(response.body());
}
// response.body(): {"orderId": "0x…", "payUrl": "…", "status": "created", …}`,
    rust: `use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Order {
    order_id: String,
    pay_url: String,
    expires_at: i64,
}

async fn create_order(client: &reqwest::Client, api_key: &str) -> reqwest::Result<Order> {
    client
        .post("${base}/orders")
        .bearer_auth(api_key)
        .json(&json!({
            "amount": "12.50",
            "reference": "order-1042",
            "successUrl": "https://yourshop.com/thanks",
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}`,
  };
}

export function readOrder(base: string): Record<Lang, string> {
  return {
    curl: `curl ${base}/orders/0x5f1c…9a2e`,
    node: `const order = await fetch(\`${base}/orders/\${orderId}\`).then((r) => r.json());
if (order.status === "paid") fulfil(order.reference);`,
    python: `order = requests.get(f"${base}/orders/{order_id}", timeout=10).json()
if order["status"] == "paid":
    fulfil(order["reference"])`,
    go: `res, err := http.Get("${base}/orders/" + orderID)
if err != nil {
	return err
}
defer res.Body.Close()
var order struct {
	Status    string \`json:"status"\`
	Reference string \`json:"reference"\`
}
if err := json.NewDecoder(res.Body).Decode(&order); err != nil {
	return err
}`,
    php: `$order = json_decode(file_get_contents("${base}/orders/" . $orderId), true);
if ($order["status"] === "paid") {
    fulfil($order["reference"]);
}`,
    ruby: `order = JSON.parse(Net::HTTP.get(URI("${base}/orders/#{order_id}")))
fulfil(order["reference"]) if order["status"] == "paid"`,
    java: `var request = HttpRequest.newBuilder(URI.create("${base}/orders/" + orderId)).GET().build();
var json = HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString()).body();`,
    rust: `let order: serde_json::Value = reqwest::get(format!("${base}/orders/{order_id}"))
    .await?
    .json()
    .await?;
let paid = order["status"] == "paid";`,
  };
}

export function verifyWebhook(): Record<Lang, string> {
  return {
    curl: `# Check a signature by hand: the HMAC-SHA256 of the raw body, as lowercase hex.
printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$UNIFORM_WEBHOOK_SECRET" | awk '{print $2}'
# It must equal the X-Uniform-Signature header.`,
    node: `import crypto from "node:crypto";
import express from "express";

const app = express();

// The raw bytes, exactly as sent: re-serialised JSON would not match the signature.
app.post("/uniform/webhook", express.raw({ type: "application/json" }), (req, res) => {
  const expected = crypto
    .createHmac("sha256", process.env.UNIFORM_WEBHOOK_SECRET)
    .update(req.body)
    .digest("hex");
  const given = req.get("X-Uniform-Signature") ?? "";
  const ok =
    given.length === expected.length &&
    crypto.timingSafeEqual(Buffer.from(given), Buffer.from(expected));
  if (!ok) return res.sendStatus(401);

  const event = JSON.parse(req.body);
  if (event.type === "order.paid") markPaid(event.reference);
  res.sendStatus(200);
});`,
    python: `import hashlib
import hmac
import os

from flask import Flask, abort, request

app = Flask(__name__)
SECRET = os.environ["UNIFORM_WEBHOOK_SECRET"].encode()

@app.post("/uniform/webhook")
def uniform_webhook():
    body = request.get_data()  # raw bytes, before any JSON parsing
    expected = hmac.new(SECRET, body, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(expected, request.headers.get("X-Uniform-Signature", "")):
        abort(401)

    event = request.get_json()
    if event["type"] == "order.paid":
        mark_paid(event["reference"])
    return "", 200`,
    go: `func uniformWebhook(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "bad body", http.StatusBadRequest)
		return
	}
	mac := hmac.New(sha256.New, []byte(os.Getenv("UNIFORM_WEBHOOK_SECRET")))
	mac.Write(body)
	expected := hex.EncodeToString(mac.Sum(nil))
	if !hmac.Equal([]byte(expected), []byte(r.Header.Get("X-Uniform-Signature"))) {
		http.Error(w, "bad signature", http.StatusUnauthorized)
		return
	}

	var event struct {
		Type      string \`json:"type"\`
		Reference string \`json:"reference"\`
	}
	json.Unmarshal(body, &event)
	if event.Type == "order.paid" {
		markPaid(event.Reference)
	}
	w.WriteHeader(http.StatusOK)
}`,
    php: `<?php
$body = file_get_contents("php://input");
$expected = hash_hmac("sha256", $body, getenv("UNIFORM_WEBHOOK_SECRET"));
$given = $_SERVER["HTTP_X_UNIFORM_SIGNATURE"] ?? "";

if (!hash_equals($expected, $given)) {
    http_response_code(401);
    exit;
}

$event = json_decode($body, true);
if ($event["type"] === "order.paid") {
    markPaid($event["reference"]);
}
http_response_code(200);`,
    ruby: `require "openssl"
require "rack/utils"
require "sinatra"

post "/uniform/webhook" do
  body = request.body.read
  expected = OpenSSL::HMAC.hexdigest("SHA256", ENV.fetch("UNIFORM_WEBHOOK_SECRET"), body)
  given = request.env["HTTP_X_UNIFORM_SIGNATURE"].to_s
  halt 401 unless Rack::Utils.secure_compare(expected, given)

  event = JSON.parse(body)
  mark_paid(event["reference"]) if event["type"] == "order.paid"
  200
end`,
    java: `import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

static boolean isFromUniform(byte[] rawBody, String signatureHeader, String secret) throws Exception {
    Mac mac = Mac.getInstance("HmacSHA256");
    mac.init(new SecretKeySpec(secret.getBytes(StandardCharsets.UTF_8), "HmacSHA256"));
    String expected = HexFormat.of().formatHex(mac.doFinal(rawBody));
    return signatureHeader != null && MessageDigest.isEqual(
        expected.getBytes(StandardCharsets.US_ASCII),
        signatureHeader.getBytes(StandardCharsets.US_ASCII));
}`,
    rust: `use hmac::{Hmac, Mac};
use sha2::Sha256;

/// True when \`signature\` (the X-Uniform-Signature header) signs \`body\` (the raw bytes).
fn is_from_uniform(body: &[u8], signature: &str, secret: &str) -> bool {
    let Ok(given) = hex::decode(signature) else { return false };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    mac.verify_slice(&given).is_ok() // constant-time
}`,
  };
}

/** The browser side: the widget, from plain HTML, from code, and from React. */
export function widget(site: string): Sample[] {
  return [
    {
      lang: "html",
      label: "HTML",
      code: `<script src="${site}/widget.js" defer></script>

<!-- POSTs to your endpoint, which answers { "payUrl": "…" } -->
<button data-uniform-checkout="/api/checkout"
        data-uniform-body='{"cartId": "c_81"}'>
  Pay with Uniform
</button>

<script>
  document.addEventListener("uniform:paid", (event) => {
    location.href = "/thanks?order=" + event.detail.orderId;
  });
</script>`,
    },
    {
      lang: "javascript",
      label: "JavaScript",
      code: `// After <script src="${site}/widget.js">. Call it straight from the click.
payButton.addEventListener("click", async () => {
  try {
    const { orderId } = await Uniform.checkout(async () => {
      const response = await fetch("/api/checkout", { method: "POST" });
      return (await response.json()).payUrl;
    });
    showThanks(orderId);
  } catch (error) {
    if (error.message !== "closed") showError(error.message);
  }
});`,
    },
    {
      lang: "tsx",
      label: "React",
      code: `"use client";
import Script from "next/script";

declare global {
  interface Window {
    Uniform: { checkout(target: string | (() => Promise<string>)): Promise<{ orderId: string }> };
  }
}

export function PayButton({ cartId }: { cartId: string }) {
  async function pay() {
    const { orderId } = await window.Uniform.checkout(async () => {
      const response = await fetch("/api/checkout", { method: "POST", body: JSON.stringify({ cartId }) });
      return (await response.json()).payUrl;
    });
    window.location.href = \`/thanks?order=\${orderId}\`;
  }

  return (
    <>
      <Script src="${site}/widget.js" strategy="afterInteractive" />
      <button onClick={pay}>Pay with Uniform</button>
    </>
  );
}`,
    },
  ];
}

export const ORDER_CREATED = `{
  "orderId": "0x5f1c3e…9a2e",
  "status": "created",
  "amount": "12.500000",
  "payUrl": "https://…/pay/0x5f1c3e…9a2e",
  "expiresAt": 1794003600
}`;

export const WEBHOOK_BODY = `POST /uniform/webhook
Content-Type: application/json
X-Uniform-Event: order.paid
X-Uniform-Signature: 7d0a3b…c41f

{
  "type": "order.paid",
  "orderId": "0x5f1c3e…9a2e",
  "reference": "order-1042",
  "amount": "12.500000",
  "currency": "USDT",
  "path": "crosschain",
  "receipts": {
    "customer": ["https://sepolia.basescan.org/tx/0x…"],
    "settlement": "https://explorer.testnet.whitechain.io/tx/0x…"
  }
}`;

export const ERROR_BODY = `HTTP/1.1 400 Bad Request

{
  "error": {
    "code": "bad_amount",
    "message": "amount must be USDT as decimal text with at most 6 decimals, like \\"12.50\\""
  }
}`;
