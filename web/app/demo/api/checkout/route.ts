// The Demo Shop's server: it makes the order on the Order Desk with its own API key, which never
// reaches the browser. The widget calls this when the customer clicks Pay.

import { randomBytes } from "node:crypto";
import { totalUnits, type Cart } from "../../catalog";

export async function POST(request: Request) {
  const key = process.env.DEMO_SHOP_API_KEY;
  if (!key) return Response.json({ error: "The demo shop isn't set up: DEMO_SHOP_API_KEY is missing." }, { status: 503 });

  const body = (await request.json().catch(() => null)) as { cart?: Cart } | null;
  const units = totalUnits(body?.cart ?? {});
  if (units === BigInt(0)) return Response.json({ error: "Your cart is empty." }, { status: 400 });

  const amount = `${units / BigInt(1_000_000)}.${(units % BigInt(1_000_000)).toString().padStart(6, "0")}`;
  const reference = `KC-${randomBytes(3).toString("hex").toUpperCase()}`;
  const response = await fetch(`${process.env.ORDER_DESK_URL}/v1/orders`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${key}` },
    body: JSON.stringify({ amount, reference }),
    cache: "no-store",
  });
  const order = await response.json().catch(() => null);
  if (!response.ok) {
    return Response.json({ error: order?.error?.message ?? "The payment service didn't answer." }, { status: 502 });
  }
  return Response.json({ orderId: order.orderId, payUrl: order.payUrl, reference });
}
