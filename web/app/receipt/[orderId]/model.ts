// What a receipt says, once: the receipt card, the dashboard's receipt window and the PDF all read it.

import { chainName, explorerTx, legToken, trimAmount } from "@/lib/chain";
import { currentLegs, type Order, type OrderStatus } from "@/lib/desk";
import { describeLegs } from "@/app/pay/[orderId]/progress";

/** Statuses that never change again: stop refreshing. */
export const FINAL: OrderStatus[] = ["paid", "refunded", "expired", "returned"];

export const STATUS_TEXT: Record<OrderStatus, string> = {
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

export function isPaid(order: Order): boolean {
  return (
    order.status === "paid" ||
    order.status === "partially_refunded" ||
    order.status === "refunded"
  );
}

export function receiptAmount(order: Order): string {
  return trimAmount(order.amount);
}

export function receiptRoute(order: Order): string {
  const legs = currentLegs(order);
  if (
    (order.path === "crosschain" || order.path === "split") &&
    legs.length > 0
  ) {
    return `${describeLegs(legs)} → USDT on Whitechain`;
  }
  return order.path === "direct" ? "USDT on Whitechain" : "Not chosen yet";
}

/** "Paid" with the payment time once paid, else "Created" with the order's creation time. */
export function receiptTime(order: Order): { label: string; unix: number } {
  return order.paidAt
    ? { label: "Paid", unix: order.paidAt }
    : { label: "Created", unix: order.createdAt };
}

export function formatTime(unix: number): string {
  return new Date(unix * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

/** Each on-chain step of the order, oldest first. */
export function receiptTransactions(
  order: Order,
): { label: string; href: string }[] {
  const lines: { label: string; href: string }[] = [];
  if (order.path === "direct" && order.payment?.explorerUrl) {
    lines.push({
      label: "Payment to the merchant · Whitechain",
      href: order.payment.explorerUrl,
    });
  }
  for (const leg of order.legs) {
    if (leg.lockTx) {
      lines.push({
        label: `Customer's ${legToken(leg.chainId, leg.token).symbol} locked · ${chainName(leg.chainId)}`,
        href: explorerTx(leg.chainId, leg.lockTx),
      });
    }
  }
  if (order.claim?.explorerUrl)
    lines.push({
      label: "Merchant paid in USDT · Whitechain",
      href: order.claim.explorerUrl,
    });
  for (const leg of order.legs) {
    if (leg.refundTx) {
      lines.push({
        label: `${legToken(leg.chainId, leg.token).symbol} returned · ${chainName(leg.chainId)}`,
        href: explorerTx(leg.chainId, leg.refundTx),
      });
    }
  }
  return lines;
}
