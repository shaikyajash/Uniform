"use client";

// The shop's orders, newest first, with refunds for Path A payments.

import { useWallets } from "@privy-io/react-auth";
import { useMemo, useState } from "react";
import { ArrowUpRight, Download, Loader2, Undo2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { chainName, shortAddress } from "@/lib/chain";
import {
  downloadCsv,
  explainDashboard,
  listOrders,
  refundNote,
  submitRefund,
  type OrderRow,
  type Session,
} from "@/lib/dashboard";
import type { OrderStatus } from "@/lib/desk";
import { cn } from "@/lib/utils";
import { Dot, Eyebrow, ORDER_STATUS, Panel, formatTime, signTypedDataWith, usdt, usePolling, walletFor } from "./bits";

const FILTERS: { label: string; statuses: OrderStatus[] }[] = [
  { label: "All", statuses: [] },
  { label: "Paid", statuses: ["paid", "partially_refunded"] },
  { label: "Refunded", statuses: ["partially_refunded", "refunded"] },
  { label: "In progress", statuses: ["created", "quoted", "paying", "source_locked", "destination_locked"] },
  { label: "Not paid", statuses: ["expired", "returned"] },
];

const REFRESH_MS = 4000;

export function Payments({ session }: { session: Session }) {
  const [filter, setFilter] = useState(0);
  const [refunding, setRefunding] = useState<OrderRow | null>(null);
  const [page, reload] = usePolling(() => listOrders(session.token, FILTERS[filter].statuses), REFRESH_MS, [
    session.token,
    filter,
  ]);
  const orders = page?.orders ?? null;

  return (
    <div className="grid gap-6">
      <Summary token={session.token} />
      <Panel
        title="Payments"
        action={
          <Button
            variant="ghost"
            size="sm"
            className="font-mono text-[10.5px] tracking-[0.08em] text-muted-foreground uppercase hover:text-foreground"
            onClick={() =>
              downloadCsv(session.token).catch((error) => toast.error(explainDashboard(error)))
            }
          >
            <Download /> CSV
          </Button>
        }
      >
        <div className="flex flex-wrap gap-1 border-b border-hair px-3 py-2.5" role="tablist" aria-label="Filter">
          {FILTERS.map((option, index) => (
            <button
              key={option.label}
              type="button"
              role="tab"
              aria-selected={filter === index}
              onClick={() => setFilter(index)}
              className={cn(
                "rounded-full px-3 py-1 text-[12.5px] transition-colors",
                filter === index ? "bg-white/[0.08] text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {option.label}
            </button>
          ))}
        </div>
        <OrdersTable orders={orders} onRefund={setRefunding} />
      </Panel>
      <RefundDialog
        session={session}
        order={refunding}
        onClose={() => setRefunding(null)}
        onSent={() => {
          setRefunding(null);
          reload();
        }}
      />
    </div>
  );
}

/** Three numbers across all paid orders: what came in, how many, and what went back. */
function Summary({ token }: { token: string }) {
  const [page] = usePolling(() => listOrders(token, ["paid", "partially_refunded", "refunded"]), REFRESH_MS * 2, [token]);
  const totals = useMemo(() => {
    if (!page) return null;
    const units = (text: string) => {
      const [whole, fraction = ""] = text.split(".");
      return BigInt(whole) * BigInt(1_000_000) + BigInt(fraction.padEnd(6, "0").slice(0, 6) || "0");
    };
    const toText = (value: bigint) => `${value / BigInt(1_000_000)}.${(value % BigInt(1_000_000)).toString().padStart(6, "0")}`;
    const received = page.orders.reduce((sum, order) => sum + units(order.amount), BigInt(0));
    const refunded = page.orders.reduce((sum, order) => sum + units(order.refunded), BigInt(0));
    return { received: toText(received), count: page.orders.length, refunded: toText(refunded) };
  }, [page]);

  const stats = [
    { label: "Received", value: totals ? usdt(totals.received) : "—", unit: "USDT" },
    { label: "Payments", value: totals ? String(totals.count) : "—", unit: "" },
    { label: "Refunded", value: totals ? usdt(totals.refunded) : "—", unit: "USDT" },
  ];
  return (
    <div className="grid grid-cols-1 overflow-hidden rounded-2xl border border-hair sm:grid-cols-3">
      {stats.map((stat, index) => (
        <div key={stat.label} className={cn("grid gap-2 px-5 py-5", index > 0 && "border-t border-hair sm:border-t-0 sm:border-l")}>
          <Eyebrow>{stat.label}</Eyebrow>
          <p className="flex items-baseline gap-2">
            <span className="font-display text-[32px] leading-none tracking-[-0.03em]">{stat.value}</span>
            {stat.unit && <span className="font-mono text-[11px] text-muted-foreground">{stat.unit}</span>}
          </p>
        </div>
      ))}
    </div>
  );
}

function OrdersTable({ orders, onRefund }: { orders: OrderRow[] | null; onRefund: (order: OrderRow) => void }) {
  if (!orders) {
    return (
      <div className="grid place-items-center py-16">
        <Loader2 className="size-4 animate-spin text-faint" />
      </div>
    );
  }
  if (orders.length === 0) {
    return (
      <div className="grid justify-items-center gap-1.5 px-6 py-16 text-center">
        <p className="text-[14.5px]">No payments here yet</p>
        <p className="max-w-sm text-[13px] leading-[1.6] text-muted-foreground">
          Orders your shop creates with its API key show up here as soon as they exist.
        </p>
      </div>
    );
  }
  return (
    <Table>
      <TableHeader>
        <TableRow className="border-hair hover:bg-transparent">
          {["Date", "Reference", "Amount", "Route", "Status", "Customer", ""].map((head, index) => (
            <TableHead
              key={index}
              className={cn(
                "h-10 font-mono text-[10px] font-normal tracking-[0.1em] text-faint uppercase first:pl-5 last:pr-5",
                head === "Amount" && "text-right",
              )}
            >
              {head}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {orders.map((order) => (
          <OrderLine key={order.orderId} order={order} onRefund={onRefund} />
        ))}
      </TableBody>
    </Table>
  );
}

function OrderLine({ order, onRefund }: { order: OrderRow; onRefund: (order: OrderRow) => void }) {
  const status = ORDER_STATUS[order.status];
  const refundable =
    order.path !== "crosschain" &&
    (order.status === "paid" || order.status === "partially_refunded") &&
    order.refundStatus !== "pending" &&
    order.refundStatus !== "sending" &&
    order.refundStatus !== "sent";
  const refundLive = order.refundStatus === "pending" || order.refundStatus === "sending" || order.refundStatus === "sent";
  return (
    <TableRow className="border-hair text-[13.5px] hover:bg-white/[0.02]">
      <TableCell className="pl-5 text-muted-foreground tabular-nums">{formatTime(order.createdAt)}</TableCell>
      <TableCell className="max-w-[180px] truncate font-mono text-[12.5px]">{order.reference}</TableCell>
      <TableCell className="text-right tabular-nums">
        {usdt(order.amount)}
        {Number(order.refunded) > 0 && (
          <span className="block text-[11.5px] text-warn/80">−{usdt(order.refunded)}</span>
        )}
      </TableCell>
      <TableCell className="text-muted-foreground">
        {order.path === "crosschain" ? "USDC → USDT" : order.path === "direct" ? "USDT" : "—"}
      </TableCell>
      <TableCell>
        <span className="inline-flex items-center gap-2">
          <Dot tone={refundLive ? "live" : status.tone} />
          {refundLive ? "Refunding" : status.text}
        </span>
        {order.refundStatus === "failed" && (
          <span className="block max-w-[220px] truncate text-[11.5px] text-bad" title={order.refundError ?? ""}>
            Refund failed{order.refundError ? `: ${order.refundError}` : ""}
          </span>
        )}
      </TableCell>
      <TableCell className="font-mono text-[12.5px] text-muted-foreground">
        {order.payer ? shortAddress(order.payer) : "—"}
      </TableCell>
      <TableCell className="pr-5">
        <div className="flex items-center justify-end gap-1">
          {refundable && (
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-foreground"
              onClick={() => onRefund(order)}
            >
              <Undo2 /> Refund
            </Button>
          )}
          <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:text-foreground" asChild>
            <a href={`/receipt/${order.orderId}`} target="_blank" rel="noreferrer" aria-label={`Receipt for ${order.reference}`}>
              <ArrowUpRight />
            </a>
          </Button>
        </div>
      </TableCell>
    </TableRow>
  );
}

/** Refund part or all of a Path A payment: the payout wallet signs a note, the Relayer sends it. */
function RefundDialog({
  session,
  order,
  onClose,
  onSent,
}: {
  session: Session;
  order: OrderRow | null;
  onClose: () => void;
  onSent: () => void;
}) {
  const { wallets } = useWallets();
  const [amount, setAmount] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [shownFor, setShownFor] = useState<string | null>(null);

  // A fresh dialog per order: the full remaining amount, no old error.
  if (order && shownFor !== order.orderId) {
    const left = Number(order.amount) - Number(order.refunded);
    setShownFor(order.orderId);
    setAmount(left.toFixed(6).replace(/0+$/, "").replace(/\.$/, ""));
    setProblem(null);
  }

  const left = order ? Number(order.amount) - Number(order.refunded) : 0;
  const wallet = walletFor(wallets, session.shop.payoutAddress);

  async function refund() {
    if (!order) return;
    if (!wallet) {
      setProblem(`Connect the payout wallet ${shortAddress(session.shop.payoutAddress)} to sign the refund.`);
      return;
    }
    setBusy(true);
    setProblem(null);
    try {
      const note = await refundNote(session.token, order.orderId, amount.trim());
      const signature = await signTypedDataWith(wallet, note.typedData);
      await submitRefund(session.token, order.orderId, note, signature);
      toast.success(`Refunding ${usdt(note.amount)} USDT`, {
        description: `To ${order.payer ? shortAddress(order.payer) : "the customer"} on ${chainName(1874)}. Uniform pays the gas.`,
      });
      onSent();
    } catch (error) {
      setProblem(explainDashboard(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={order !== null}
      onOpenChange={(open) => {
        if (!open && !busy) {
          setShownFor(null);
          onClose();
        }
      }}
    >
      <DialogContent className="border-hair-strong bg-gradient-to-b from-panel-2 to-panel sm:max-w-[420px]">
        <DialogHeader>
          <Eyebrow>Refund · {order?.reference}</Eyebrow>
          <DialogTitle className="font-display text-2xl font-normal tracking-[-0.02em]">Send money back</DialogTitle>
          <DialogDescription className="leading-[1.65]">
            The USDT goes from your payout wallet back to the customer who paid. Your wallet signs a note; Uniform sends
            it and pays the gas.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void refund();
          }}
        >
          <Label htmlFor="refund-amount" className="font-mono text-[10px] font-normal tracking-[0.1em] text-faint uppercase">
            Amount · up to {usdt(left.toFixed(6))} USDT
          </Label>
          <div className="relative">
            <Input
              id="refund-amount"
              inputMode="decimal"
              autoComplete="off"
              value={amount}
              onChange={(event) => setAmount(event.target.value)}
              className="h-12 border-hair-strong bg-black/30 pr-16 font-display text-xl tracking-[-0.02em]"
            />
            <span className="absolute top-1/2 right-4 -translate-y-1/2 font-mono text-[11px] text-muted-foreground">USDT</span>
          </div>
          {!wallet && (
            <p className="text-[12.5px] leading-[1.6] text-warn">
              Your payout wallet {shortAddress(session.shop.payoutAddress)} isn&apos;t connected in this browser.
            </p>
          )}
          {problem && (
            <p role="alert" className="border-l-2 border-bad pl-3 text-[13px] leading-[1.6] text-label">
              {problem}
            </p>
          )}
          <DialogFooter className="mt-3">
            <Button
              type="submit"
              disabled={busy || !amount.trim()}
              className="h-11 w-full rounded-full bg-white font-mono text-[11.5px] tracking-[0.08em] text-black uppercase hover:bg-white/90"
            >
              {busy ? <Loader2 className="animate-spin" /> : null}
              {busy ? "Check your wallet" : "Sign refund"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
