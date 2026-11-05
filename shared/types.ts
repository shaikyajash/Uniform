export type Hex = `0x${string}`;

export type OrderStatus =
  | "created"
  | "quoted"
  | "customer_locked"
  | "shop_locked"
  | "paid"
  | "partially_refunded"
  | "refunded"
  | "expired";

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
  | { type: "Locked"; chainId: number; lockId: Hex; orderId: Hex; usdcAmount: string; txHash: Hex }
  | { type: "Filled"; orderId: Hex; shopLockId: Hex; amount: string; txHash: Hex }
  | { type: "Settled"; orderId: Hex; amount: string; txHash: Hex }
  | { type: "Claimed"; chainId: number; lockId: Hex; txHash: Hex }
  | { type: "Refunded"; chainId: number; lockId: Hex; txHash: Hex };

export type WebhookType = "order.paid" | "order.refunded" | "order.expired";
