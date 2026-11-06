export type Hex = `0x${string}`;

/** Every status an order can be in. Stored in orders.status. */
export const ORDER_STATUSES = [
  "created", // order made by the shop, nobody has paid
  "quoted", // customer opened the page and got a price and the data to sign
  "paying", // signature received, transaction sent, waiting for the chain event
  "source_locked", // cross-chain: every customer leg is locked (Locked events)
  "destination_locked", // cross-chain: Bob's USDT is locked for the shop (Filled)
  "paid", // the shop has the USDT (Paid or Settled)
  "partially_refunded", // the shop gave back part of the amount
  "refunded", // the shop gave back the full amount
  "expired", // the quote ran out before any money moved
  "returned", // cross-chain timed out and the customer's locked money went back
] as const;
export type OrderStatus = (typeof ORDER_STATUSES)[number];

/** Allowed moves. The Order Desk rejects any move not listed here. */
export const ORDER_TRANSITIONS: Record<OrderStatus, readonly OrderStatus[]> = {
  created: ["quoted", "expired"],
  quoted: ["quoted", "paying", "expired"], // a new quote can replace an old one
  paying: ["paid", "source_locked", "quoted"], // back to quoted if the transaction fails
  source_locked: ["destination_locked", "returned"],
  destination_locked: ["paid", "returned"],
  paid: ["partially_refunded", "refunded"],
  partially_refunded: ["partially_refunded", "refunded"],
  refunded: [],
  expired: [],
  returned: [],
};

export function canMove(from: OrderStatus, to: OrderStatus): boolean {
  return ORDER_TRANSITIONS[from].includes(to);
}

/** Status of one cross-chain leg (legs.status). */
export const LEG_STATUSES = ["pending", "locked", "claimed", "returned"] as const;
export type LegStatus = (typeof LEG_STATUSES)[number];

/** Status of one refund (refunds.status). */
export const REFUND_STATUSES = ["pending", "sent", "failed"] as const;
export type RefundStatus = (typeof REFUND_STATUSES)[number];

export type PayPath = "direct" | "crosschain" | "split";

export interface Order {
  orderId: Hex;
  merchantId: string;
  reference: string;
  amountUsdt: string; // decimal string, 6 decimals
  status: OrderStatus;
  path?: PayPath;
  hashlock?: Hex;
  legs: Leg[];
  settlementTx?: Hex;
  createdAt: number;
}

export interface Leg {
  chainId: number;
  tokenIn: "USDC" | "EURC" | "ETH" | "USDT";
  amountIn: string;
  minUsdcOut?: string;
  lockId?: Hex;
  status: LegStatus;
  lockTx?: Hex;
  claimTx?: Hex;
}

export interface LockParams {
  orderId: Hex;
  receiver: Hex;
  minUsdcOut: bigint;
  hashlock: Hex;
  expiry: bigint;
  outToken: Hex;
}

export type ChainEvent =
  | { type: "Paid"; paymentId: Hex; orderId: Hex; payer: Hex; amount: string; txHash: Hex }
  | { type: "DirectRefunded"; paymentId: Hex; orderId: Hex; amount: string; totalRefunded: string; txHash: Hex }
  | { type: "Locked"; chainId: number; lockId: Hex; orderId: Hex; token: Hex; amount: string; txHash: Hex }
  | { type: "Filled"; orderId: Hex; destinationLockId: Hex; amount: string; txHash: Hex }
  | { type: "Settled"; orderId: Hex; amount: string; txHash: Hex }
  | { type: "Claimed"; chainId: number; lockId: Hex; txHash: Hex }
  | { type: "Refunded"; chainId: number; lockId: Hex; txHash: Hex };

export type WebhookType = "order.paid" | "order.refunded" | "order.expired";
