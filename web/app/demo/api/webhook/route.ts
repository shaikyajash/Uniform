// The Demo Store's webhook: Uniform's Notifier POSTs here when an order is paid, returned or
// expired. The merchant checks the signature before believing it:
//   X-Uniform-Signature: hex(HMAC_SHA256(body, webhook secret))
// Verified deliveries are kept in this server's memory so the merchant page can show them.

import { createHmac, timingSafeEqual } from "node:crypto";

type Delivery = { type: string; reference: string; amount: string; path: string; receivedAt: number };

const store = globalThis as unknown as { __demoWebhooks?: Map<string, Delivery> };
const deliveries = (store.__demoWebhooks ??= new Map<string, Delivery>());

export async function POST(request: Request) {
  const secret = process.env.DEMO_WEBHOOK_SECRET;
  const body = await request.text();
  const given = request.headers.get("x-uniform-signature") ?? "";
  if (!secret) return new Response("webhook secret not set", { status: 503 });

  const expected = createHmac("sha256", secret).update(body).digest("hex");
  const valid =
    given.length === expected.length && timingSafeEqual(Buffer.from(given, "utf8"), Buffer.from(expected, "utf8"));
  if (!valid) return new Response("bad signature", { status: 401 });

  const event = JSON.parse(body) as { type: string; orderId: string; reference: string; amount: string; path: string };
  deliveries.set(event.orderId.toLowerCase(), {
    type: event.type,
    reference: event.reference,
    amount: event.amount,
    path: event.path,
    receivedAt: Math.floor(Date.now() / 1000),
  });
  return new Response(null, { status: 204 });
}

export async function GET(request: Request) {
  const orderId = new URL(request.url).searchParams.get("orderId")?.toLowerCase() ?? "";
  return Response.json({ delivery: deliveries.get(orderId) ?? null });
}
