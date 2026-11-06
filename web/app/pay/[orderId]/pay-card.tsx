"use client";

// Path A: the customer pays USDT on Whitechain. They sign one note (no gas); the Relayer sends it.

import { usePrivy, useSignTypedData, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import { parseUnits, type Address } from "viem";
import { formatUsdt, shopChain, shortAddress, trimAmount, usdtBalance } from "@/lib/chain";
import { explain, getOrder, getQuote, submitDirect, type Order, type Payment } from "@/lib/desk";
import styles from "./pay.module.css";

/** How often to look at the order while its payment is on the way. */
const ORDER_POLL_MS = 2_000;
/** How often to look at the balance while it is too low (the customer may be topping up). */
const BALANCE_POLL_MS = 5_000;

type Step = "idle" | "quoting" | "signing" | "submitting";

const STEP_LABEL: Record<Exclude<Step, "idle">, string> = {
  quoting: "Preparing…",
  signing: "Waiting for your signature…",
  submitting: "Sending…",
};

export function PayCard({ orderId }: { orderId: string }) {
  const { ready, authenticated, login, logout } = usePrivy();
  const { wallets } = useWallets();
  const { signTypedData } = useSignTypedData();

  const [order, setOrder] = useState<Order | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [balance, setBalance] = useState<{ owner: string; units: bigint } | null>(null);
  const [step, setStep] = useState<Step>("idle");
  const [error, setError] = useState<string | null>(null);
  const now = useNow();

  const address = (wallets.find((wallet) => wallet.walletClientType === "privy")?.address ?? null) as Address | null;
  const myBalance = balance && balance.owner === address ? balance.units : null;

  const refresh = useCallback(async () => {
    try {
      setOrder(await getOrder(orderId));
      setLoadError(null);
    } catch (problem) {
      setLoadError(explain(problem));
    }
  }, [orderId]);

  const readBalance = useCallback(async () => {
    if (!address) return;
    try {
      setBalance({ owner: address, units: await usdtBalance(address) });
    } catch {
      // Keep the last value; the next read may work.
    }
  }, [address]);

  useEffect(() => {
    let current = true;
    getOrder(orderId).then(
      (loaded) => current && setOrder(loaded),
      (problem) => current && setLoadError(explain(problem)),
    );
    return () => {
      current = false;
    };
  }, [orderId]);

  // While the payment is on its way, follow it.
  useEffect(() => {
    if (order?.status !== "paying") return;
    const timer = setInterval(refresh, ORDER_POLL_MS);
    return () => clearInterval(timer);
  }, [order?.status, refresh]);

  const needed = order ? parseUnits(order.amount, shopChain.usdtDecimals) : null;
  const enough = myBalance !== null && needed !== null && myBalance >= needed;

  // Read the balance when the wallet appears, and again when a payment finishes.
  const paymentStatus = order?.payment?.status;
  useEffect(() => {
    if (!address) return;
    let current = true;
    usdtBalance(address).then(
      (units) => current && setBalance({ owner: address, units }),
      () => {},
    );
    return () => {
      current = false;
    };
  }, [address, paymentStatus]);

  // While the balance is too low, look again now and then: the customer may be topping up.
  useEffect(() => {
    if (!address || enough) return;
    const timer = setInterval(readBalance, BALANCE_POLL_MS);
    return () => clearInterval(timer);
  }, [address, enough, readBalance]);

  async function pay() {
    if (!order || !address) return;
    setError(null);
    try {
      setStep("quoting");
      const quote = await getQuote(orderId, address);
      const { typedData } = quote.toSign[0];
      setStep("signing");
      const { signature } = await signTypedData(
        { ...typedData, domain: { ...typedData.domain, chainId: Number(typedData.domain.chainId) } },
        {
          address,
          uiOptions: {
            title: `Pay ${trimAmount(order.amount)} USDT`,
            description: `To ${order.shop.name}. No gas needed: Uniform pays the network fee.`,
            buttonText: "Sign and pay",
          },
        },
      );
      setStep("submitting");
      await submitDirect(orderId, quote.quoteId, signature);
    } catch (problem) {
      setError(explain(problem));
    } finally {
      setStep("idle");
      await refresh();
    }
  }

  if (!order) {
    return (
      <section className={styles.card} aria-busy={!loadError}>
        {loadError ? <p className={styles.error}>{loadError}</p> : <p className={styles.muted}>Loading order…</p>}
      </section>
    );
  }

  const amount = trimAmount(order.amount);
  const payment = order.payment;
  const confirmed = order.status === "paid" || payment?.status === "confirmed";
  const onItsWay = order.status === "paying" && !confirmed;
  const secondsLeft = order.expiresAt - now;
  const expired =
    order.status === "expired" || ((order.status === "created" || order.status === "quoted") && secondsLeft <= 0);

  return (
    <section className={styles.card}>
      <header className={styles.header}>
        <span className={styles.label}>Pay</span>
        <h1 className={styles.shop}>{order.shop.name}</h1>
        <p className={styles.muted}>Order {order.reference}</p>
      </header>

      <div className={styles.amount}>
        <span className={styles.value}>{amount}</span>
        <span className={styles.unit}>USDT</span>
      </div>
      <p className={styles.network}>
        <i className={styles.dot} /> on Whitechain · no gas needed
      </p>

      {confirmed ? (
        <Done order={order} amount={amount} />
      ) : onItsWay ? (
        <OnItsWay payment={payment} />
      ) : expired ? (
        <p className={styles.notice}>The time to pay this order has run out. Ask the shop for a new link.</p>
      ) : !ready ? (
        <button className={styles.primary} disabled>
          Loading…
        </button>
      ) : !authenticated ? (
        <>
          <button className={styles.primary} onClick={login}>
            Log in with email to pay
          </button>
          <p className={styles.fine}>We create a wallet for you. Nothing to install.</p>
        </>
      ) : !address ? (
        <p className={styles.muted}>Creating your wallet…</p>
      ) : (
        <>
          <div className={styles.wallet}>
            <span>
              From <code title={address}>{shortAddress(address)}</code>
            </span>
            <span>Balance {myBalance === null ? "…" : `${formatUsdt(myBalance)} USDT`}</span>
            <button className={styles.link} onClick={logout}>
              Not you?
            </button>
          </div>

          {myBalance !== null && !enough && (
            <p className={styles.notice}>
              You need {amount} USDT on Whitechain. Send USDT to <CopyText text={address} /> and this page updates by
              itself.
            </p>
          )}
          {payment?.status === "failed" && (
            <p className={styles.error}>
              The last try didn&apos;t go through{payment.failedReason ? `: ${payment.failedReason}` : ""}. Nothing was
              taken.
            </p>
          )}
          {error && <p className={styles.error}>{error}</p>}

          <button className={styles.primary} disabled={step !== "idle" || !enough} onClick={pay}>
            {step === "idle" ? `Pay ${amount} USDT` : STEP_LABEL[step]}
          </button>
          <p className={styles.fine}>Expires in {formatCountdown(secondsLeft)}</p>
        </>
      )}
    </section>
  );
}

function OnItsWay({ payment }: { payment: Payment | null }) {
  if (payment?.status === "failed") {
    return (
      <p className={styles.error}>
        This payment couldn&apos;t be sent{payment.failedReason ? `: ${payment.failedReason}` : ""}.
      </p>
    );
  }
  const text =
    payment?.status === "sent"
      ? "Waiting for Whitechain to confirm…"
      : payment?.status === "sending"
        ? "Sending your payment…"
        : "Getting your payment ready…";
  return (
    <div className={styles.progress} role="status">
      <span className={styles.spinner} aria-hidden="true" />
      <p>{text}</p>
      {payment?.explorerUrl && (
        <a className={styles.link} href={payment.explorerUrl} target="_blank" rel="noreferrer">
          View on explorer ↗
        </a>
      )}
    </div>
  );
}

function Done({ order, amount }: { order: Order; amount: string }) {
  return (
    <div className={styles.done} role="status">
      <span className={styles.check} aria-hidden="true">
        ✓
      </span>
      <p className={styles.doneTitle}>Payment confirmed</p>
      <p className={styles.muted}>
        {order.shop.name} received {amount} USDT on Whitechain.
      </p>
      {order.payment?.explorerUrl && (
        <a className={styles.link} href={order.payment.explorerUrl} target="_blank" rel="noreferrer">
          View on explorer ↗
        </a>
      )}
    </div>
  );
}

function CopyText({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className={styles.copy}
      title={text}
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => setCopied(true));
      }}
    >
      {copied ? "copied ✓" : shortAddress(text)}
    </button>
  );
}

/** Unix seconds, ticking once a second. */
function useNow(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function formatCountdown(seconds: number): string {
  const safe = Math.max(0, seconds);
  return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, "0")}`;
}
