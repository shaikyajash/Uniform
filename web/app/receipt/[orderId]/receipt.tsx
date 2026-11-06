"use client";

import { useEffect, useState, type ReactNode } from "react";
import { ArrowUpRight } from "lucide-react";
import { CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { chainName, explorerTx, trimAmount } from "@/lib/chain";
import { currentLegs, explain, getOrder, type Order, type OrderStatus } from "@/lib/desk";
import { describeLegs } from "@/app/pay/[orderId]/progress";
import { CopyText, FinePrint, PayCardSkeleton, PayShell, Problem } from "@/app/pay/[orderId]/parts";

/** Statuses that never change again: stop refreshing. */
const FINAL: OrderStatus[] = ["paid", "refunded", "expired", "returned"];
const REFRESH_MS = 4000;

const STATUS_TEXT: Record<OrderStatus, string> = {
  created: "Waiting for payment",
  quoted: "Waiting for payment",
  paying: "Payment on its way",
  source_locked: "Payment on its way",
  destination_locked: "Payment on its way",
  paid: "Paid",
  partially_refunded: "Partly refunded",
  refunded: "Refunded",
  expired: "Expired, not paid",
  returned: "Not paid, money returned",
};

export function Receipt({ orderId }: { orderId: string }) {
  const [order, setOrder] = useState<Order | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = () =>
      getOrder(orderId).then(
        (next) => {
          if (stop) return;
          setOrder(next);
          setProblem(null);
          if (!FINAL.includes(next.status)) timer = setTimeout(load, REFRESH_MS);
        },
        (error) => {
          if (stop) return;
          setProblem(explain(error));
          timer = setTimeout(load, REFRESH_MS);
        },
      );
    void load();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [orderId]);

  if (!order) return problem ? <PayShell><CardContent><Problem>{problem}</Problem></CardContent></PayShell> : <PayCardSkeleton />;

  const amount = trimAmount(order.amount);
  const legs = currentLegs(order);
  const paid = order.status === "paid" || order.status === "partially_refunded" || order.status === "refunded";

  return (
    <PayShell>
      <div className="flex flex-col gap-5">
        <CardHeader className="gap-4">
          <div className="flex justify-between gap-3 font-mono text-[10.5px] tracking-[0.08em] text-label">
            <span>RECEIPT</span>
            <span>ORDER {order.reference}</span>
          </div>
          <Separator className="bg-hair" />
          <div className="grid gap-1">
            <h1 className="text-2xl font-normal tracking-[-0.02em]">{order.shop.name}</h1>
            <p className="flex items-center gap-2 text-[13.5px] text-muted-foreground">
              <span
                aria-hidden="true"
                className={`size-1.5 rounded-full ${paid ? "bg-ok" : FINAL.includes(order.status) ? "bg-faint" : "animate-pulse bg-white"}`}
              />
              {STATUS_TEXT[order.status]}
            </p>
          </div>
        </CardHeader>

        <CardContent className="grid gap-5">
          <p className="flex items-baseline gap-2.5">
            <span className="bg-gradient-to-b from-white via-zinc-300 to-zinc-400 bg-clip-text font-display text-[44px] leading-none tracking-[-0.035em] text-transparent">
              {amount}
            </span>
            <span className="font-mono text-sm text-muted-foreground">USDT</span>
          </p>

          <dl className="grid gap-0 border-y border-hair text-[13.5px]">
            <Row label="Paid to">
              <CopyText text={order.shop.address} />
            </Row>
            {order.payer && (
              <Row label="Paid by">
                <CopyText text={order.payer} />
              </Row>
            )}
            <Row label="Route">
              {(order.path === "crosschain" || order.path === "split") && legs.length > 0
                ? `${describeLegs(legs)} → USDT on Whitechain`
                : order.path === "direct"
                  ? "USDT on Whitechain"
                  : "Not chosen yet"}
            </Row>
            <Row label={order.paidAt ? "Paid" : "Created"}>{formatTime(order.paidAt ?? order.createdAt)}</Row>
            <Row label="Order id">
              <CopyText text={order.orderId} />
            </Row>
          </dl>

          <Transactions order={order} />

          <FinePrint>Every line above can be checked on the chain</FinePrint>
        </CardContent>
      </div>
    </PayShell>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-b border-hair py-2.5 last:border-b-0">
      <dt className="font-mono text-[10px] tracking-[0.1em] text-faint uppercase">{label}</dt>
      <dd className="text-right text-label">{children}</dd>
    </div>
  );
}

/** Each on-chain step of the order, oldest first. */
function Transactions({ order }: { order: Order }) {
  const lines: { label: string; href: string }[] = [];
  if (order.path === "direct" && order.payment?.explorerUrl) {
    lines.push({ label: "Payment to the shop · Whitechain", href: order.payment.explorerUrl });
  }
  for (const leg of order.legs) {
    if (leg.lockTx) lines.push({ label: `Customer's USDC locked · ${chainName(leg.chainId)}`, href: explorerTx(leg.chainId, leg.lockTx) });
  }
  if (order.claim?.explorerUrl) lines.push({ label: "Shop paid in USDT · Whitechain", href: order.claim.explorerUrl });
  for (const leg of order.legs) {
    if (leg.refundTx) lines.push({ label: `USDC returned · ${chainName(leg.chainId)}`, href: explorerTx(leg.chainId, leg.refundTx) });
  }
  if (lines.length === 0) return null;
  return (
    <div className="grid gap-2">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">TRANSACTIONS</small>
      <ul className="grid gap-1.5">
        {lines.map((line) => (
          <li key={line.href}>
            <a
              href={line.href}
              target="_blank"
              rel="noreferrer"
              className="group flex items-center justify-between gap-3 rounded-lg border border-hair px-3 py-2.5 text-[13.5px] text-label transition-colors hover:border-hair-strong hover:bg-white/[0.03]"
            >
              {line.label}
              <ArrowUpRight className="size-3.5 text-faint transition-colors group-hover:text-white" aria-hidden="true" />
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}

function formatTime(unix: number): string {
  return new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}
