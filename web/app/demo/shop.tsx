"use client";

// The Demo Store's page: three products, a cart, and Uniform's widget for checkout. Styled as a
// different site on purpose, so the checkout reads as something the merchant plugged in.

import { useEffect, useState } from "react";
import { ArrowUpRight, Check, Minus, Plus } from "lucide-react";
import Link from "next/link";
import { CATALOG, formatUnits6, totalUnits, type Cart } from "./catalog";

declare global {
  interface Window {
    Uniform?: { checkout: (target: string | (() => Promise<string>)) => Promise<{ orderId: string }> };
  }
}

type Paid = { orderId: string; reference: string };
type Delivery = { type: string; receivedAt: number } | null;

export function Store() {
  const [cart, setCart] = useState<Cart>({ kettle: 1 });
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [paid, setPaid] = useState<Paid | null>(null);
  const count = Object.values(cart).reduce((sum, n) => sum + n, 0);
  const total = totalUnits(cart);

  function change(id: string, by: number) {
    setCart((current) => ({ ...current, [id]: Math.max(0, Math.min(9, (current[id] ?? 0) + by)) }));
  }

  function checkout() {
    if (!window.Uniform) {
      setProblem("The checkout is still loading. Try again in a second.");
      return;
    }
    setProblem(null);
    setBusy(true);
    let reference = "";
    // Called straight from the click, so the browser allows the payment window.
    window.Uniform.checkout(async () => {
      const response = await fetch("/demo/api/checkout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cart }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? "Couldn't start checkout.");
      reference = body.reference;
      return body.payUrl as string;
    }).then(
      ({ orderId }) => {
        setPaid({ orderId, reference });
        setCart({});
        setBusy(false);
      },
      (error: Error) => {
        setBusy(false);
        if (error.message !== "closed") setProblem(error.message);
      },
    );
  }

  return (
    <>
      <div className="border-b border-[#1f1b16]/10 bg-[#1f1b16] px-4 py-2 text-center font-mono text-[11px] tracking-[0.06em] text-[#f3eee5]/80">
        DEMO STORE · PAYMENTS RUN ON TEST NETWORKS (TESTNET USDT AND USDC) ·{" "}
        <Link href="/" className="whitespace-nowrap text-[#f3eee5] underline underline-offset-4 hover:no-underline">
          BACK TO UNIFORM
        </Link>
      </div>

      <header className="mx-auto flex max-w-6xl items-center justify-between px-5 py-6 md:px-8">
        <a href="/demo" className="font-[family-name:var(--font-merchant)] text-[30px] leading-none tracking-[-0.01em]">
          Kettle &amp; Co.
        </a>
        <span className="text-sm text-[#1f1b16]/60">Cart · {count}</span>
      </header>

      <main className="mx-auto grid max-w-6xl gap-10 px-5 pb-20 md:grid-cols-[1fr_340px] md:px-8">
        <section>
          <h1 className="max-w-xl font-[family-name:var(--font-merchant)] text-[44px] leading-[1.05] tracking-[-0.015em] md:text-[56px]">
            Slow coffee, made at home.
          </h1>
          <p className="mt-3 max-w-md text-[15px] leading-relaxed text-[#1f1b16]/65">
            A small kit for pour-over. Pay with stablecoins from any supported chain, with just an email.
          </p>

          <ul className="mt-10 grid gap-4 sm:grid-cols-3">
            {CATALOG.map((product) => {
              const n = cart[product.id] ?? 0;
              return (
                <li key={product.id} className="flex flex-col rounded-xl bg-[#fbf8f2] p-4 ring-1 ring-[#1f1b16]/8">
                  <div className="grid aspect-[4/3] place-items-center rounded-lg bg-[#ebe3d6]">
                    <span className="font-[family-name:var(--font-merchant)] text-[64px] leading-none text-[#1f1b16]/25">
                      {product.name[0]}
                    </span>
                  </div>
                  <p className="mt-4 text-[15px] font-medium">{product.name}</p>
                  <p className="text-[13px] text-[#1f1b16]/55">{product.detail}</p>
                  <div className="mt-4 flex items-center justify-between">
                    <span className="font-mono text-[13px]">{product.price} USDT</span>
                    {n === 0 ? (
                      <button
                        type="button"
                        onClick={() => change(product.id, 1)}
                        className="rounded-full bg-[#1f1b16] px-3.5 py-1.5 text-[12.5px] text-[#f3eee5] hover:bg-[#1f1b16]/85"
                      >
                        Add
                      </button>
                    ) : (
                      <Stepper n={n} onLess={() => change(product.id, -1)} onMore={() => change(product.id, 1)} />
                    )}
                  </div>
                </li>
              );
            })}
          </ul>

          <HowItsAdded />
        </section>

        <aside className="h-fit rounded-xl bg-[#fbf8f2] p-5 ring-1 ring-[#1f1b16]/8 md:sticky md:top-6">
          {paid ? (
            <Thanks paid={paid} onAgain={() => setPaid(null)} />
          ) : (
            <>
              <h2 className="font-[family-name:var(--font-merchant)] text-[26px] leading-none">Your cart</h2>
              {count === 0 ? (
                <p className="mt-4 text-[14px] text-[#1f1b16]/55">Nothing here yet.</p>
              ) : (
                <ul className="mt-4 grid gap-2.5">
                  {CATALOG.filter((product) => (cart[product.id] ?? 0) > 0).map((product) => (
                    <li key={product.id} className="flex items-center justify-between gap-3 text-[14px]">
                      <span>
                        {product.name} <span className="text-[#1f1b16]/45">× {cart[product.id]}</span>
                      </span>
                      <Stepper
                        n={cart[product.id] ?? 0}
                        onLess={() => change(product.id, -1)}
                        onMore={() => change(product.id, 1)}
                      />
                    </li>
                  ))}
                </ul>
              )}
              <div className="mt-5 flex items-baseline justify-between border-t border-[#1f1b16]/10 pt-4">
                <span className="text-[14px] text-[#1f1b16]/60">Total</span>
                <span className="font-mono text-[18px]">{formatUnits6(total)} USDT</span>
              </div>
              <button
                type="button"
                disabled={busy || total === BigInt(0)}
                onClick={checkout}
                className="mt-4 h-11 w-full rounded-full bg-[#1f1b16] text-[14px] font-medium text-[#f3eee5] transition hover:bg-[#1f1b16]/85 disabled:opacity-40"
              >
                {busy ? "Paying…" : "Pay with Uniform"}
              </button>
              <p className="mt-2.5 text-center text-[12px] text-[#1f1b16]/50">
                No wallet app, no gas. Log in with an email.
              </p>
              {problem && <p className="mt-3 rounded-lg bg-[#b4432f]/10 px-3 py-2 text-[13px] text-[#8c3322]">{problem}</p>}
            </>
          )}
        </aside>
      </main>
    </>
  );
}

function Stepper({ n, onLess, onMore }: { n: number; onLess: () => void; onMore: () => void }) {
  return (
    <span className="inline-flex items-center rounded-full ring-1 ring-[#1f1b16]/15">
      <button type="button" aria-label="One less" onClick={onLess} className="grid size-7 place-items-center hover:bg-[#1f1b16]/5">
        <Minus className="size-3.5" />
      </button>
      <span className="w-5 text-center font-mono text-[12.5px]">{n}</span>
      <button type="button" aria-label="One more" onClick={onMore} className="grid size-7 place-items-center hover:bg-[#1f1b16]/5">
        <Plus className="size-3.5" />
      </button>
    </span>
  );
}

/** After paying: the order, and whether the merchant's server got the signed webhook. */
function Thanks({ paid, onAgain }: { paid: Paid; onAgain: () => void }) {
  const [delivery, setDelivery] = useState<Delivery>(null);
  useEffect(() => {
    if (delivery) return;
    const look = () =>
      fetch(`/demo/api/webhook?orderId=${paid.orderId}`, { cache: "no-store" })
        .then((response) => response.json())
        .then((body: { delivery: Delivery }) => body.delivery && setDelivery(body.delivery))
        .catch(() => {});
    const timer = setInterval(look, 2_000);
    const first = setTimeout(look, 0);
    return () => {
      clearInterval(timer);
      clearTimeout(first);
    };
  }, [paid.orderId, delivery]);

  return (
    <div>
      <span className="grid size-10 place-items-center rounded-full bg-[#1f1b16] text-[#f3eee5]">
        <Check className="size-5" />
      </span>
      <h2 className="mt-4 font-[family-name:var(--font-merchant)] text-[28px] leading-tight">Thank you. Order paid.</h2>
      <p className="mt-1 font-mono text-[12.5px] text-[#1f1b16]/55">{paid.reference}</p>
      <dl className="mt-5 grid gap-2.5 border-t border-[#1f1b16]/10 pt-4 text-[13.5px]">
        <div className="flex justify-between gap-3">
          <dt className="text-[#1f1b16]/55">Checkout</dt>
          <dd>Paid</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-[#1f1b16]/55">Our server</dt>
          <dd>{delivery ? "Webhook received, signature checked" : "Waiting for the webhook…"}</dd>
        </div>
      </dl>
      <div className="mt-5 flex items-center justify-between">
        <a href={`/receipt/${paid.orderId}`} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-[13.5px] underline underline-offset-4">
          Receipt <ArrowUpRight className="size-3.5" />
        </a>
        <button type="button" onClick={onAgain} className="text-[13.5px] text-[#1f1b16]/60 hover:text-[#1f1b16]">
          Shop again
        </button>
      </div>
    </div>
  );
}

const SNIPPET = `<!-- 1. On the page -->
<script src="https://uniform.example/widget.js" defer></script>

<script>
  payButton.onclick = () =>
    Uniform.checkout(async () => {
      const r = await fetch("/api/checkout", { method: "POST", body: cartJson });
      return (await r.json()).payUrl;
    }).then(({ orderId }) => showThanks(orderId));
</script>

// 2. On the merchant's server: make the order with the secret API key
const order = await fetch(UNIFORM + "/v1/orders", {
  method: "POST",
  headers: { authorization: "Bearer " + process.env.UNIFORM_API_KEY },
  body: JSON.stringify({ amount: "1.20", reference: "KC-1042" }),
}).then((r) => r.json());   // { orderId, payUrl }

// 3. Trust only the signed webhook: X-Uniform-Signature = HMAC-SHA256(body, secret)`;

function HowItsAdded() {
  return (
    <details className="group mt-14 rounded-xl ring-1 ring-[#1f1b16]/10">
      <summary className="flex cursor-pointer list-none items-center justify-between px-5 py-4 text-[14px]">
        <span>How this store added Uniform</span>
        <span className="font-mono text-[12px] text-[#1f1b16]/50 group-open:hidden">show</span>
        <span className="hidden font-mono text-[12px] text-[#1f1b16]/50 group-open:inline">hide</span>
      </summary>
      <pre className="overflow-x-auto border-t border-[#1f1b16]/10 bg-[#1f1b16] px-5 py-4 font-mono text-[12px] leading-relaxed text-[#f3eee5]/85">
        {SNIPPET}
      </pre>
    </details>
  );
}
