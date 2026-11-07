// The merchant dashboard's calls to the Order Desk, and the signed-in session.
//
// The session token (`sess_...`) comes from signing a one-time message with the merchant's payout
// wallet. It is kept in this tab's sessionStorage (gone when the tab closes) and sent as a Bearer
// token. A 401 means it ran out: the dashboard signs out.

import { useSyncExternalStore } from "react";
import { DeskError, type OrderStatus, type TypedData } from "./desk";

/* ---------- Session ---------- */

export type Session = {
  token: string;
  expiresAt: number;
  merchant: { name: string; payoutAddress: string };
};

const KEY = "uniform.dashboard.session";
const listeners = new Set<() => void>();
let cached: { raw: string | null; session: Session | null } = { raw: null, session: null };

function read(): Session | null {
  let raw: string | null = null;
  try {
    raw = sessionStorage.getItem(KEY);
  } catch {
    return null;
  }
  if (raw !== cached.raw) {
    let session: Session | null = null;
    try {
      session = raw ? (JSON.parse(raw) as Session) : null;
    } catch {
      session = null;
    }
    cached = { raw, session };
  }
  const session = cached.session;
  return session && session.expiresAt * 1000 > Date.now() ? session : null;
}

function write(session: Session | null) {
  try {
    if (session) sessionStorage.setItem(KEY, JSON.stringify(session));
    else sessionStorage.removeItem(KEY);
  } catch {
    // Storage blocked: the session lasts until the page reloads.
  }
  listeners.forEach((listener) => listener());
}

/** The signed-in session, or null. `undefined` while rendering on the server. */
export function useSession(): Session | null | undefined {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    read,
    () => undefined,
  );
}

export const signOutLocally = () => write(null);

/* ---------- Calls ---------- */

async function call<T>(path: string, init: RequestInit & { token?: string } = {}): Promise<T> {
  const { token, ...rest } = init;
  const response = await fetch(`/desk/v1${path}`, {
    ...rest,
    headers: {
      "content-type": "application/json",
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...rest.headers,
    },
    cache: "no-store",
  });
  if (response.status === 401 && token) {
    write(null);
  }
  if (response.status === 204) return undefined as T;
  const body = await response.json().catch(() => null);
  if (!response.ok) {
    const error = body?.error;
    throw new DeskError(error?.code ?? `http_${response.status}`, error?.message ?? response.statusText);
  }
  return body as T;
}

const post = (body: unknown, token?: string): RequestInit & { token?: string } => ({
  method: "POST",
  body: JSON.stringify(body ?? {}),
  token,
});

/**
 * Step 1 of signing in: the message the payout wallet signs. `merchant` says whether the wallet
 * already is one; if not, ask for a name and call again with `createName` to sign up.
 */
export const challenge = (address: string, createName?: string) =>
  call<{ nonce: string; message: string; expiresAt: number; merchant: boolean }>(
    "/auth/challenge",
    post(createName ? { address, createName } : { address }),
  );

/** A new merchant's secrets, returned once at sign-up. */
export type Created = { apiKey: string; webhookSecret: string };

// Held in memory only (never in storage), for the dashboard to show once after sign-up.
let welcome: Created | null = null;

/** The secrets from a sign-up that just happened, once; null afterwards. */
export function takeWelcome(): Created | null {
  const value = welcome;
  welcome = null;
  return value;
}

/** Step 2: the signature for a session. Saves it, which signs the dashboard in. */
export async function verify(nonce: string, signature: string): Promise<Session> {
  const { created, ...session } = await call<Session & { created?: Created }>("/auth/verify", post({ nonce, signature }));
  welcome = created ?? null;
  write(session);
  return session;
}

export async function signOut(token: string) {
  await call<void>("/auth/logout", post({}, token)).catch(() => undefined);
  write(null);
}

export type OrderRow = {
  orderId: string;
  reference: string;
  /** USDT as decimal text, "12.000000". */
  amount: string;
  status: OrderStatus;
  path: "direct" | "crosschain" | "split" | null;
  payer: string | null;
  createdAt: number;
  paidAt: number | null;
  /** USDT given back so far. */
  refunded: string;
  /** The latest refund not yet confirmed: pending, sending, sent or failed. */
  refundStatus: "pending" | "sending" | "sent" | "failed" | null;
  refundError: string | null;
};

export const listOrders = (token: string, statuses?: OrderStatus[]) =>
  call<{ orders: OrderRow[]; next: number | null }>(
    `/orders${statuses?.length ? `?status=${statuses.join(",")}` : ""}`,
    { token },
  );

/** The CSV, fetched with the token (a plain link could not send it), then saved. */
export async function downloadCsv(token: string) {
  const response = await fetch("/desk/v1/payments.csv", { headers: { authorization: `Bearer ${token}` } });
  if (!response.ok) throw new DeskError(`http_${response.status}`, "Could not export the payments");
  const url = URL.createObjectURL(await response.blob());
  const link = Object.assign(document.createElement("a"), { href: url, download: "uniform-payments.csv" });
  link.click();
  URL.revokeObjectURL(url);
}

export type Settings = {
  name: string;
  payoutAddress: string;
  webhookUrl: string | null;
  webhookSecret: string;
  createdAt: number;
};

export const getShop = (token: string) => call<Settings>("/merchant", { token });

export const updateShop = (token: string, webhookUrl: string | null) =>
  call<Settings>("/merchant", { method: "PATCH", body: JSON.stringify({ webhookUrl }), token });

export const rotateWebhookSecret = (token: string) => call<{ secret: string }>("/merchant/webhook-secret", post({}, token));

export const rotateApiKey = (token: string) => call<{ secret: string }>("/merchant/api-key", post({}, token));

export type Delivery = {
  id: number;
  orderId: string;
  reference: string;
  event: "order.paid" | "order.refunded" | "order.expired";
  url: string;
  status: "pending" | "delivered" | "failed";
  attempts: number;
  lastStatus: number | null;
  lastError: string | null;
  nextAttemptAt: number;
  createdAt: number;
  deliveredAt: number | null;
  body: unknown;
};

export const listWebhooks = (token: string) => call<Delivery[]>("/webhooks", { token });

export const retryWebhook = (token: string, id: number) => call<void>(`/webhooks/${id}/retry`, post({}, token));

export type RefundNote = { amount: string; validAfter: number; validBefore: number; typedData: TypedData };

export const refundNote = (token: string, orderId: string, amount: string) =>
  call<RefundNote>(`/orders/${orderId}/refund-note`, post({ amount }, token));

export const submitRefund = (token: string, orderId: string, note: RefundNote, signature: string) =>
  call<{ jobId: number }>(
    `/orders/${orderId}/refunds`,
    post({ amount: note.amount, validAfter: note.validAfter, validBefore: note.validBefore, signature }, token),
  );

/** Order Desk error codes, in the merchant's words. */
export function explainDashboard(error: unknown): string {
  if (error instanceof DeskError) {
    switch (error.code) {
      case "bad_name":
        return "Give your business a name, up to 60 characters.";
      case "not_a_merchant":
        return "No merchant is paid to this wallet. Sign in with the wallet your payouts go to.";
      case "challenge_expired":
        return "That sign-in message ran out. Try again.";
      case "bad_signature":
        return error.message.includes("payout") ? error.message : "The signature didn't match this wallet. Try again.";
      case "refund_in_progress":
        return "A refund for this order is already on its way.";
      case "not_refundable":
        return `This order can't be refunded: ${error.message.replace(/^the order is /, "it is ")}.`;
      case "refund_unavailable":
        return "This order has no payment to refund.";
      case "note_expired":
        return "The refund note ran out. Try again.";
      case "bad_amount":
      case "bad_webhook_url":
        return error.message.charAt(0).toUpperCase() + error.message.slice(1) + ".";
      case "unauthorized":
        return "Your session ended. Sign in again.";
    }
    return error.message;
  }
  if (error instanceof Error && /reject|denied|cancel/i.test(error.message)) return "You cancelled in your wallet.";
  return "Something went wrong. Try again.";
}
