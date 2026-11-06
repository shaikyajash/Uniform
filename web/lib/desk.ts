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
  merchant: { name: string; address: string };
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
  /** Cross-chain: Bob's lock for the merchant, once filled. */
  destinationLockId: string | null;
  /** Cross-chain: the Relayer claiming the merchant's lock with the secret. */
  claim: Payment | null;
};

export type Leg = {
  chainId: number;
  /** The token's address: USDC, or a priced token (map it with `legToken`). */
  token: string;
  /** Base units, in that token's decimals. */
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
  path: "direct" | "crosschain" | "split";
  expiresAt: number;
  total: string;
  fee: string;
  toSign: { chainId: number; token: string; amount: string; typedData: TypedData; lockId?: string }[];
};

/** A Calibur batch (EIP-712): approve the token to the SourceLock, then lock it. */
export type BatchTypedData = {
  domain: { name: string; version: string; chainId: string | number; verifyingContract: string; salt: string };
  types: Record<string, { name: string; type: string }[]>;
  primaryType: string;
  message: {
    batchedCall: { calls: { to: string; value: string; data: string }[]; revertOnFailure: boolean };
    nonce: string;
    keyHash: string;
    executor: string;
    deadline: string;
  };
};

/** A quote for a priced token (WETH, WBTC): one Calibur batch to sign. `total` is in that token. */
export type BatchQuote = Omit<Quote, "toSign"> & {
  toSign: {
    signing: "batch";
    chainId: number;
    /** Symbol, e.g. "WBTC". */
    token: string;
    tokenAddress: string;
    decimals: number;
    /** In whole tokens, as decimal text ("0.00001363"): `parseUnits(amount, decimals)` gives base units. */
    amount: string;
    lockId: string;
    calibur: string;
    typedData: BatchTypedData;
  }[];
};

/**
 * A signed EIP-7702 authorization ("run Calibur's code at my address"), as POST /legs takes it.
 * `r` and `s` are 0x hex (32 bytes); `yParity` is 0 or 1; `nonce` is the wallet's transaction nonce.
 */
export type Authorization = {
  chainId: number;
  address: string;
  nonce: number;
  yParity: number;
  r: string;
  s: string;
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

/** One chain of a split payment: how much USDC (decimal text) to lock there; one leg may leave it out. */
export type SplitLeg = { chainId: number; amount?: string };

/**
 * Path A without `crosschain`. With `{hashlock, chainId}`: a USDC note on that chain under the
 * hashlock. With `{hashlock, legs}` (A8 split): one note per chain, all under the one hashlock.
 */
/** With `{hashlock, chainId, token}`: a priced token (e.g. "WBTC") on that chain, as one Calibur batch. */
export function getQuote(
  orderId: string,
  payer: string,
  crosschain: { hashlock: string; chainId: number; token: string },
): Promise<BatchQuote>;
export function getQuote(
  orderId: string,
  payer: string,
  crosschain?: { hashlock: string; chainId: number } | { hashlock: string; legs: SplitLeg[] },
): Promise<Quote>;
export function getQuote(orderId: string, payer: string, crosschain?: object) {
  return call<Quote | BatchQuote>(`/orders/${orderId}/quote`, {
    method: "POST",
    body: JSON.stringify({ payer, ...crosschain }),
  });
}

/**
 * The signed notes of a cross-chain quote, one per `toSign` entry and in its order. A priced
 * token's batch sends `{quoteId, signatures: [batchSignature], authorization?}`, the authorization
 * only when the wallet isn't delegated to Calibur yet.
 */
export const submitLegs = (orderId: string, quoteId: string, signatures: string[], authorization?: Authorization) =>
  call<{ status: OrderStatus; jobId: number | null; lockId: string; locks?: { chainId: number; lockId: string; jobId: number | null }[] }>(
    `/orders/${orderId}/legs`,
    {
      method: "POST",
      body: JSON.stringify(
        authorization
          ? { quoteId, signatures, authorization }
          : signatures.length === 1
            ? { quoteId, signature: signatures[0] }
            : { quoteId, signatures },
      ),
    },
  );

/**
 * The customer's locks of the latest attempt: an order keeps the legs of earlier quotes too, and
 * every lock of one attempt (one chain, or a split's chains) shares one expiry.
 */
export function currentLegs(order: Order): Leg[] {
  const last = order.legs[order.legs.length - 1];
  return last ? order.legs.filter((leg) => leg.expiry === last.expiry) : [];
}

/** The leg Bob's destination lock id is built from: the first by (chainId, lockId) (A8). */
export function anchorLeg(legs: Leg[]): Leg | null {
  return (
    [...legs].sort((a, b) => a.chainId - b.chainId || a.lockId.toLowerCase().localeCompare(b.lockId.toLowerCase()))[0] ??
    null
  );
}

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
        return "The time to pay this order has run out. Ask the merchant for a new link.";
      case "order_not_payable":
        return "This order can't be paid right now.";
      case "bad_signature":
        return "The signature didn't match. Tap Pay to try again.";
      case "not_found":
        return "We couldn't find this order. Check the link.";
      case "crosschain_unavailable":
        return "Paying from another chain isn't available right now. Pay with USDT on Whitechain instead.";
      case "bad_legs":
        return "Those amounts don't add up for a split payment. Refresh and try again.";
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
