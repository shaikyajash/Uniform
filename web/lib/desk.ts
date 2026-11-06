// Calls to the Order Desk. They go to /desk/v1/..., which next.config.ts forwards to the Order Desk.

export type OrderStatus =
  | "created"
  | "quoted"
  | "paying"
  | "source_locked"
  | "destination_locked"
  | "paid"
  | "partially_refunded"
  | "refunded"
  | "expired"
  | "returned";

export type JobStatus = "pending" | "sending" | "sent" | "confirmed" | "failed";

export type Payment = {
  status: JobStatus;
  txHash: string | null;
  explorerUrl: string | null;
  failedReason: string | null;
};

export type Order = {
  orderId: string;
  shop: { name: string; address: string };
  reference: string;
  /** USDT as decimal text, e.g. "12.000000". */
  amount: string;
  status: OrderStatus;
  path: "direct" | "crosschain" | "split" | null;
  payer: string | null;
  createdAt: number;
  expiresAt: number;
  paidAt: number | null;
  /** The customer's payment (Path A) or lock (cross-chain), as the Relayer sends it. */
  payment: Payment | null;
  /** Cross-chain: keccak256 of the customer's secret. */
  hashlock: string | null;
  /** Cross-chain: the customer's locks. `lockId` is the only id to trust. */
  legs: Leg[];
  /** Cross-chain: Bob's lock for the shop, once filled. */
  destinationLockId: string | null;
  /** Cross-chain: the Relayer claiming the shop's lock with the secret. */
  claim: Payment | null;
};

export type Leg = {
  chainId: number;
  token: string;
  /** Base units (6 decimals). */
  amount: string;
  lockId: string;
  expiry: number;
  status: "pending" | "locked" | "claimed" | "returned";
  lockTx: string | null;
  claimTx: string | null;
  refundTx: string | null;
};

export type TypedData = {
  domain: { name: string; version: string; chainId: string | number; verifyingContract: string };
  types: Record<string, { name: string; type: string }[]>;
  primaryType: string;
  message: Record<string, string>;
};

export type Quote = {
  quoteId: string;
  path: "direct" | "crosschain";
  expiresAt: number;
  total: string;
  fee: string;
  toSign: { chainId: number; token: string; amount: string; typedData: TypedData; lockId?: string }[];
};

/** An error the Order Desk answered with: `{ error: { code, message } }`. */
export class DeskError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/desk/v1${path}`, {
    ...init,
    headers: { "content-type": "application/json", ...init?.headers },
    cache: "no-store",
  });
  const body = await response.json().catch(() => null);
  if (!response.ok) {
    const error = body?.error;
    throw new DeskError(error?.code ?? `http_${response.status}`, error?.message ?? response.statusText);
  }
  return body as T;
}

export const getOrder = (orderId: string) => call<Order>(`/orders/${orderId}`);

/** Path A without `crosschain`; with it, a USDC note on that chain under the hashlock. */
export const getQuote = (orderId: string, payer: string, crosschain?: { hashlock: string; chainId: number }) =>
  call<Quote>(`/orders/${orderId}/quote`, { method: "POST", body: JSON.stringify({ payer, ...crosschain }) });

export const submitLegs = (orderId: string, quoteId: string, signature: string) =>
  call<{ status: OrderStatus; jobId: number; lockId: string }>(`/orders/${orderId}/legs`, {
    method: "POST",
    body: JSON.stringify({ quoteId, signature }),
  });

export const revealSecret = (orderId: string, secret: string) =>
  call<{ status: OrderStatus; jobId: number }>(`/orders/${orderId}/secret`, {
    method: "POST",
    body: JSON.stringify({ secret }),
  });

export const submitDirect = (orderId: string, quoteId: string, signature: string) =>
  call<{ status: OrderStatus; jobId: number }>(`/orders/${orderId}/direct`, {
    method: "POST",
    body: JSON.stringify({ quoteId, signature }),
  });

/** What to tell the customer for each Order Desk error code. */
export function explain(error: unknown): string {
  if (error instanceof DeskError) {
    switch (error.code) {
      case "already_submitted":
        return "Your payment is already on its way.";
      case "quote_expired":
        return "That took a little too long. Tap Pay to sign again.";
      case "order_expired":
        return "The time to pay this order has run out. Ask the shop for a new link.";
      case "order_not_payable":
        return "This order can't be paid right now.";
      case "bad_signature":
        return "The signature didn't match. Tap Pay to try again.";
      case "not_found":
        return "We couldn't find this order. Check the link.";
      case "crosschain_unavailable":
        return "Paying from another chain isn't available right now. Pay with USDT on Whitechain instead.";
      case "wrong_secret":
        return "That key doesn't match this payment. Sign again with the same wallet.";
      default:
        return error.message;
    }
  }
  const text = error instanceof Error ? error.message : String(error);
  if (/reject|denied|cancel|closed/i.test(text)) return "You cancelled the signature. Nothing was paid.";
  return "Something went wrong. Please try again.";
}
