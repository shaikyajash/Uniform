"use client";

import { useEffect, useState, type ReactNode } from "react";
import { ArrowLeft, ArrowUpRight, Download, Loader2 } from "lucide-react";
import Link from "next/link";
import { toast } from "sonner";
import { useEmbedded } from "@/app/_ui/embedded";
import { CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { explain, getOrder, type Order } from "@/lib/desk";
import {
  CopyText,
  FinePrint,
  PayCardSkeleton,
  PayShell,
  Problem,
} from "@/app/pay/[orderId]/parts";
import {
  FINAL,
  STATUS_TEXT,
  formatTime,
  isPaid,
  receiptAmount,
  receiptRoute,
  receiptTime,
  receiptTransactions,
} from "./model";
import { downloadReceipt } from "./pdf";

const REFRESH_MS = 4000;

/** The order, refreshed until it reaches a status that never changes again. */
export function useLiveOrder(orderId: string): {
  order: Order | null;
  problem: string | null;
} {
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
          if (!FINAL.includes(next.status))
            timer = setTimeout(load, REFRESH_MS);
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

  return { order, problem };
}

/** `/receipt/<orderId>`, alone or inside the checkout card. */
export function Receipt({ orderId }: { orderId: string }) {
  const { order, problem } = useLiveOrder(orderId);
  const embedded = useEmbedded();

  // Opened inside the checkout card: ask the widget to keep the card open while it is read.
  useEffect(() => {
    if (embedded) window.parent.postMessage({ type: "uniform:stay" }, "*");
  }, [embedded]);

  if (!order) {
    return problem ? (
      <PayShell>
        <CardContent>
          <Problem>{problem}</Problem>
        </CardContent>
      </PayShell>
    ) : (
      <PayCardSkeleton />
    );
  }

  return (
    <PayShell>
      <ReceiptDetails
        order={order}
        top={
          embedded && (
            <Link
              href={`/pay/${order.orderId}`}
              className="inline-flex w-fit items-center gap-1.5 pt-1 pb-1 font-mono text-[10.5px] tracking-[0.08em] text-muted-foreground uppercase hover:text-foreground"
            >
              <ArrowLeft className="size-3.5" aria-hidden="true" /> Back
            </Link>
          )
        }
      />
    </PayShell>
  );
}

/** The receipt's lines, for the receipt card and the dashboard's receipt window. */
export function ReceiptDetails({
  order,
  top,
}: {
  order: Order;
  top?: ReactNode;
}) {
  const paid = isPaid(order);
  const time = receiptTime(order);

  return (
    <div className="flex flex-col gap-5">
      <CardHeader className="gap-4">
        {top}
        <div className="flex justify-between gap-3 font-mono text-[10.5px] tracking-[0.08em] text-label">
          <span>RECEIPT</span>
          <span>ORDER {order.reference}</span>
        </div>
        <Separator className="bg-hair" />
        <div className="grid gap-1">
          <h1 className="text-2xl font-normal tracking-[-0.02em]">
            {order.merchant.name}
          </h1>
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
            {receiptAmount(order)}
          </span>
          <span className="font-mono text-sm text-muted-foreground">USDT</span>
        </p>

        <dl className="grid gap-0 border-y border-hair text-[13.5px]">
          <Row label="Paid to">
            <CopyText text={order.merchant.address} />
          </Row>
          {order.payer && (
            <Row label="Paid by">
              <CopyText text={order.payer} />
            </Row>
          )}
          <Row label="Route">{receiptRoute(order)}</Row>
          <Row label={time.label}>{formatTime(time.unix)}</Row>
          <Row label="Order id">
            <CopyText text={order.orderId} />
          </Row>
        </dl>

        <Transactions order={order} />

        <DownloadButton order={order} />

        <FinePrint>Every line above can be checked on the chain</FinePrint>
      </CardContent>
    </div>
  );
}

function DownloadButton({ order }: { order: Order }) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      disabled={busy}
      onClick={() => {
        setBusy(true);
        downloadReceipt(order)
          .catch(() => toast.error("The PDF couldn't be made. Try again."))
          .finally(() => setBusy(false));
      }}
      className="inline-flex h-10 items-center justify-center gap-2 rounded-full border border-hair-strong font-mono text-[11px] tracking-[0.08em] text-label uppercase transition-colors hover:border-white/40 hover:bg-white/[0.04] hover:text-foreground disabled:opacity-60"
    >
      {busy ? (
        <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
      ) : (
        <Download className="size-3.5" aria-hidden="true" />
      )}
      Download PDF
    </button>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-b border-hair py-2.5 last:border-b-0">
      <dt className="font-mono text-[10px] tracking-[0.1em] text-faint uppercase">
        {label}
      </dt>
      <dd className="text-right text-label">{children}</dd>
    </div>
  );
}

function Transactions({ order }: { order: Order }) {
  const lines = receiptTransactions(order);
  if (lines.length === 0) return null;
  return (
    <div className="grid gap-2">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">
        TRANSACTIONS
      </small>
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
              <ArrowUpRight
                className="size-3.5 text-faint transition-colors group-hover:text-white"
                aria-hidden="true"
              />
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
