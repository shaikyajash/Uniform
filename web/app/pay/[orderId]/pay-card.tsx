"use client";

// The pay page. The customer logs in with email (Privy gives them a wallet), picks what to pay
// with, and signs. They never pay gas: the Relayer sends every transaction.
//
// - USDT on Whitechain (Path A): one signed note; the Relayer sends it to DirectPay.
// - USDC on a customer chain (cross-chain): one signature makes the payment's key (the secret,
//   kept only in this page's memory), a second signs the USDC note that locks it for the
//   provider. Once the provider has locked USDT for the merchant, this page checks that lock on
//   chain itself and only then hands over the key, which pays the merchant.

import { usePrivy, useSignTypedData, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { formatUnits, parseUnits, type Address, type Hex } from "viem";
import { Check, Loader2 } from "lucide-react";
import {
  chainName,
  checkShopLock,
  customerChain,
  customerChains,
  explorerTx,
  shopChain,
  shortAddress,
  trimAmount,
  usdcBalance,
  usdtBalance,
  withProviderFee,
} from "@/lib/chain";
import {
  anchorLeg,
  currentLegs,
  explain,
  getOrder,
  getQuote,
  revealSecret,
  submitDirect,
  submitLegs,
  type Order,
  type Quote,
  type Payment,
  type TypedData,
} from "@/lib/desk";
import { secretFromSignature, secretTypedData } from "@/lib/secret";
import { useEmbedded } from "@/app/_ui/embedded";
import { EmailLogin } from "./email-login";
import { Badge } from "@/components/ui/badge";
import { CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  CopyIcon,
  CopyText,
  ExplorerLink,
  FinePrint,
  Notice,
  PayShell,
  PrimaryButton,
  Problem,
  Reveal,
  SkeletonBody,
  Stat,
  Swap,
  TextButton,
  formatCountdown,
  useNow,
} from "./parts";
import { SourcePicker, encodeSource, type Source, type SourceOption, type SplitPart } from "./source-picker";
import { Progress, describeLegs } from "./progress";

/** How often to look at the order while a payment is on its way. */
const ORDER_POLL_MS = 2_000;
/** How often to look at balances while none covers the order (the customer may be topping up). */
const BALANCE_POLL_MS = 5_000;
/** Orders that are moving and worth following. */
const MOVING = new Set(["paying", "source_locked", "destination_locked"]);

type Step = "idle" | "keying" | "quoting" | "signing" | "submitting";

const STEP_LABEL: Record<Exclude<Step, "idle">, string> = {
  keying: "Confirm in your wallet…",
  quoting: "Preparing…",
  signing: "Waiting for your signature…",
  submitting: "Sending…",
};

/** The Order Desk's smallest split leg: 0.1 USDC (A8). */
const MIN_SPLIT_LEG = BigInt(100_000);

/** The payment's key, in memory only. */
type Key = { orderId: string; secret: Hex; hashlock: Hex };

/** Balances by source, read for one wallet. `null` while unknown. */
type Balances = { owner: Address; units: Record<string, bigint | null> };

export function PayCard({ orderId }: { orderId: string }) {
  const embedded = useEmbedded();
  const { ready, authenticated, logout, user } = usePrivy();
  const { wallets, ready: walletsReady } = useWallets();
  const { signTypedData } = useSignTypedData();

  const [order, setOrder] = useState<Order | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [balances, setBalances] = useState<Balances | null>(null);
  const [picked, setPicked] = useState<Source | null>(null);
  const [step, setStep] = useState<Step>("idle");
  const [error, setError] = useState<string | null>(null);
  const [key, setKey] = useState<Key | null>(null);
  const [lockProblem, setLockProblem] = useState<string | null>(null);
  const revealing = useRef(false);
  const now = useNow();

  const wallet = wallets.find((candidate) => candidate.walletClientType === "privy") ?? null;
  const address = (wallet?.address ?? null) as Address | null;
  const units = order ? parseUnits(order.amount, shopChain.usdtDecimals) : null;

  const refresh = useCallback(async () => {
    try {
      setOrder(await getOrder(orderId));
      setLoadError(null);
    } catch (problem) {
      setLoadError(explain(problem));
    }
  }, [orderId]);

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

  // While the payment moves, follow it.
  const moving = order ? MOVING.has(order.status) : false;
  useEffect(() => {
    if (!moving) return;
    const timer = setInterval(refresh, ORDER_POLL_MS);
    return () => clearInterval(timer);
  }, [moving, refresh]);

  // Every way to pay, with what it costs and whether the wallet has it.
  const options = useMemo(() => paymentOptions(units, balances, address), [units, balances, address]);

  const anyEnough = options.some((option) => option.balance !== null && option.balance >= option.cost);
  const firstEnough = options.find((option) => option.balance !== null && option.balance >= option.cost);
  // The picked row as built from the latest balances: a split's parts follow them.
  const pickedNow = picked ? (options.find((option) => sameSource(option.source, picked))?.source ?? null) : null;
  const source = pickedNow ?? firstEnough?.source ?? options[0]?.source ?? null;
  const chosen = options.find((option) => sameSource(option.source, source)) ?? null;
  const chosenEnough = chosen !== null && chosen.balance !== null && chosen.balance >= chosen.cost;

  const readBalances = useCallback(async () => {
    if (!address) return;
    const reads: [string, Promise<bigint>][] = [
      ["usdt", usdtBalance(address)],
      ...customerChains.map((chain): [string, Promise<bigint>] => [`usdc:${chain.id}`, usdcBalance(chain, address)]),
    ];
    const settled = await Promise.allSettled(reads.map(([, read]) => read));
    setBalances((previous) => {
      const next: Record<string, bigint | null> = {};
      reads.forEach(([name], index) => {
        const result = settled[index];
        const kept = previous?.owner === address ? (previous.units[name] ?? null) : null;
        next[name] = result.status === "fulfilled" ? result.value : kept;
      });
      return { owner: address, units: next };
    });
  }, [address]);

  // Read balances when the wallet appears and when a payment finishes or fails.
  const paymentStatus = order?.payment?.status;
  useEffect(() => {
    if (!address) return;
    const timer = setTimeout(readBalances, 0);
    return () => clearTimeout(timer);
  }, [address, paymentStatus, readBalances]);

  // While nothing covers the order, look again now and then: the customer may be topping up.
  useEffect(() => {
    if (!address || anyEnough) return;
    const timer = setInterval(readBalances, BALANCE_POLL_MS);
    return () => clearInterval(timer);
  }, [address, anyEnough, readBalances]);

  // Tell the merchant's page once the order is paid: the widget shows us in a card (an iframe); an
  // older widget opened us as a pop-up. Only the order id is sent, so any listener may read it.
  const paid = order?.status === "paid";
  useEffect(() => {
    if (!paid) return;
    const shopPage = window.parent !== window ? window.parent : window.opener;
    shopPage?.postMessage({ type: "uniform:paid", orderId }, "*");
  }, [paid, orderId]);

  /** Signs this order's fixed message: the same order always gives the same key. */
  const makeKey = useCallback(async (): Promise<Key> => {
    if (!address || units === null || !order) throw new Error("not ready");
    const { signature } = await signTypedData(secretTypedData(order.orderId as Hex, units), {
      address,
      // Our Pay button is the confirmation, on the full page and in the widget alike: no Privy window.
      uiOptions: { showWalletUIs: false, title: "", description: "", buttonText: "" },
    });
    const made = { orderId: order.orderId, ...secretFromSignature(signature as Hex) };
    setKey(made);
    return made;
  }, [address, units, order, signTypedData]);

  // Cross-chain: once the provider has locked USDT for the merchant, check that lock on chain and
  // hand over the key. Runs again on every poll until the claim is under way.
  // A split has a lock per chain; Bob's lock for the merchant is named after the first (A8).
  const leg = order ? anchorLeg(currentLegs(order)) : null;
  const keyFits = key !== null && order !== null && key.orderId === order.orderId && key.hashlock === order.hashlock;
  const claimLive = order?.claim != null && order.claim.status !== "failed";
  useEffect(() => {
    if (order?.status !== "destination_locked" || !keyFits || claimLive || !leg || units === null || !key) return;
    if (revealing.current) return;
    revealing.current = true;
    void (async () => {
      try {
        const check = await checkShopLock({
          srcChainId: leg.chainId,
          srcLockId: leg.lockId as Hex,
          hashlock: key.hashlock,
          merchant: order.merchant.address as Address,
          amountUnits: units,
        });
        if (!check.ok) {
          setLockProblem(check.reason);
          return;
        }
        setLockProblem(null);
        await revealSecret(order.orderId, key.secret);
        await refresh();
      } catch (problem) {
        setLockProblem(explain(problem));
      } finally {
        revealing.current = false;
      }
    })();
  }, [order, keyFits, claimLive, leg, units, key, refresh]);

  /** No Privy window: the customer's Pay click is the confirmation (A8: one click for every note). */
  async function signNote(typedData: TypedData, title: string, description: string): Promise<string> {
    if (!address) throw new Error("no wallet");
    const data = { ...typedData, domain: { ...typedData.domain, chainId: Number(typedData.domain.chainId) } };
    const options = {
      address,
      uiOptions: { showWalletUIs: false, title, description, buttonText: "Sign and pay" },
    };
    try {
      return (await signTypedData(data, options)).signature;
    } catch (problem) {
      // Some wallets sign typed data only for the chain they are on.
      if (!wallet || !/chain/i.test(String(problem))) throw problem;
      await wallet.switchChain(data.domain.chainId);
      return (await signTypedData(data, options)).signature;
    }
  }

  async function payDirect() {
    if (!order || !address) return;
    setStep("quoting");
    const quote = await getQuote(orderId, address);
    setStep("signing");
    const signature = await signNote(
      quote.toSign[0].typedData,
      `Pay ${trimAmount(order.amount)} USDT`,
      `To ${order.merchant.name}. No gas needed: Uniform pays the network fee.`,
    );
    setStep("submitting");
    await submitDirect(orderId, quote.quoteId, signature);
  }

  async function payCrossChain(chainId: number) {
    if (!order || !address || !customerChain(chainId)) return;
    setStep("keying");
    const made = keyFits && key ? key : await makeKey();
    setStep("quoting");
    const quote = await getQuote(orderId, address, { hashlock: made.hashlock, chainId });
    const note = quote.toSign[0];
    if (!noteFits(note, chainId, address)) throw new Error("The payment details didn't check out. Nothing was signed.");
    setStep("signing");
    const signature = await signNote(
      note.typedData,
      `Pay ${trimAmount(quote.total)} USDC`,
      `From ${chainName(chainId)}. Held for the provider until ${order.merchant.name} is paid; it comes back to you if not.`,
    );
    setStep("submitting");
    await submitLegs(orderId, quote.quoteId, [signature]);
  }

  /** A8 split: part of the USDC from each chain, every signature behind this one Pay click. */
  async function paySplit(parts: SplitPart[]) {
    if (!order || !address || parts.length < 2) return;
    setStep("keying");
    const made = keyFits && key ? key : await makeKey();
    setStep("quoting");
    const [first, ...rest] = parts;
    const quote = await getQuote(orderId, address, {
      hashlock: made.hashlock,
      legs: [{ chainId: first.chainId, amount: formatUnits(first.amount, 6) }, ...rest.map((part) => ({ chainId: part.chainId }))],
    });
    // Every note must be ours, one per planned chain, within what the wallet holds there, and
    // together exactly the quoted total.
    const held = balances?.owner === address ? balances.units : {};
    const values = quote.toSign.map((note) => BigInt(note.typedData.message.value ?? "0"));
    const fits =
      quote.path === "split" &&
      quote.toSign.length === parts.length &&
      parts.every((part) => quote.toSign.some((note) => note.chainId === part.chainId)) &&
      quote.toSign.every((note, index) => noteFits(note, note.chainId, address) && values[index] <= (held[`usdc:${note.chainId}`] ?? BigInt(0))) &&
      values.reduce((sum, value) => sum + value, BigInt(0)) === parseUnits(quote.total, 6);
    if (!fits) throw new Error("The payment details didn't check out. Nothing was signed.");
    setStep("signing");
    // One after another: each is a separate signature in the same wallet.
    const signatures = await quote.toSign.reduce<Promise<string[]>>(
      async (done, note, index) => [
        ...(await done),
        await signNote(
          note.typedData,
          `Pay ${formatUsdc(values[index])} USDC`,
          `From ${chainName(note.chainId)}, part of ${trimAmount(quote.total)} USDC to ${order.merchant.name}.`,
        ),
      ],
      Promise.resolve([]),
    );
    setStep("submitting");
    await submitLegs(orderId, quote.quoteId, signatures);
  }

  async function pay() {
    if (!source) return;
    setError(null);
    try {
      if (source.kind === "usdt") await payDirect();
      else if (source.kind === "usdc") await payCrossChain(source.chainId);
      else await paySplit(source.parts);
    } catch (problem) {
      setError(explain(problem));
    } finally {
      setStep("idle");
      await refresh();
    }
  }

  /** After a reload mid-payment: sign the same message again to get the same key back. */
  async function finish() {
    setError(null);
    try {
      const made = await makeKey();
      if (made.hashlock !== order?.hashlock) {
        setError("This wallet didn't start this payment. Log in with the email you paid with.");
      }
    } catch (problem) {
      setError(explain(problem));
    }
  }

  if (!order) {
    return (
      <PayShell busy={!loadError}>
        <Swap id={loadError ? "error" : "loading"}>
          {loadError ? (
            <CardContent>
              <Problem>{loadError}</Problem>
            </CardContent>
          ) : (
            <SkeletonBody />
          )}
        </Swap>
      </PayShell>
    );
  }

  const amount = trimAmount(order.amount);
  const payment = order.payment;
  const crossChain = order.path === "crosschain";
  const secondsLeft = order.expiresAt - now;
  const expired =
    order.status === "expired" || ((order.status === "created" || order.status === "quoted") && secondsLeft <= 0);
  const directConfirmed = !crossChain && payment?.status === "confirmed";
  const view =
    order.status === "paid" || directConfirmed
      ? "done"
      : order.status === "returned"
        ? "returned"
        : moving && crossChain
          ? "progress"
          : moving
            ? "on-its-way"
            : expired
              ? "expired"
              : !ready
                ? "starting"
                : !authenticated
                  ? "log-in"
                  : !address
                    ? "creating-wallet"
                    : "pay";

  return (
    <PayShell>
      <Swap id="order">
        <div className="flex flex-col gap-5 embedded:gap-4">
          {embedded ? (
            <CompactSummary merchant={order.merchant.name} reference={order.reference} amount={amount} secondsLeft={secondsLeft} />
          ) : (
            <CardHeader className="gap-4">
              <div className="flex justify-between gap-3 font-mono text-[10.5px] tracking-[0.08em] text-label">
                <span>PAY</span>
                <span>ORDER {order.reference}</span>
              </div>
              <Separator className="bg-hair" />
              <h1 className="text-2xl font-normal tracking-[-0.02em]">{order.merchant.name}</h1>
            </CardHeader>
          )}

          <CardContent className="relative grid gap-4">
            {!embedded && (
              <div className="grid gap-2">
                <p className="flex items-baseline gap-2.5">
                  <span className="bg-gradient-to-b from-white via-zinc-300 to-zinc-400 bg-clip-text font-display text-[56px] leading-none tracking-[-0.035em] text-transparent">
                    {amount}
                  </span>
                  <span className="font-mono text-sm text-muted-foreground">USDT</span>
                </p>
                <p className="flex flex-wrap items-center gap-2 text-[14.5px] leading-[1.7] text-muted-foreground">
                  The merchant is paid on Whitechain
                  <Badge variant="outline" className="border-hair-strong font-mono text-[10px] tracking-[0.08em] text-label">
                    NO GAS NEEDED
                  </Badge>
                </p>
              </div>
            )}

            <Swap id={view}>
              <div className="grid gap-4 embedded:-mx-(--card-spacing) embedded:gap-3 embedded:border-t embedded:border-hair embedded:px-(--card-spacing) embedded:pt-4">
                {view === "done" && <Done order={order} amount={amount} />}
                {view === "returned" && <Returned order={order} />}
                {view === "on-its-way" && <OnItsWay payment={payment} />}
                {view === "progress" && (
                  <>
                    <Progress order={order} amount={amount} />
                    <Reveal show={order.status === "destination_locked" && !keyFits && !claimLive}>
                      <div className="grid gap-3">
                        <Notice>
                          The merchant&apos;s USDT is locked and waiting. Confirm once more to release it: this page
                          doesn&apos;t keep your payment key.
                        </Notice>
                        <PrimaryButton onClick={finish}>Finish payment</PrimaryButton>
                      </div>
                    </Reveal>
                    <Reveal show={lockProblem !== null && order.status === "destination_locked"}>
                      <FinePrint>Checking the merchant&apos;s lock: {lockProblem}</FinePrint>
                    </Reveal>
                    <Reveal show={error !== null}>
                      <Problem>{error}</Problem>
                    </Reveal>
                  </>
                )}
                {view === "expired" && (
                  <Notice>The time to pay this order has run out. Ask the merchant for a new link.</Notice>
                )}
                {view === "starting" && <PrimaryButton disabled>Loading…</PrimaryButton>}
                {view === "log-in" && <EmailLogin />}
                {view === "creating-wallet" && (
                  <WalletWait accountWallet={user?.wallet?.address ?? null} walletsReady={walletsReady} embedded={embedded} />
                )}
                {view === "pay" && address && (
                  <>
                    <div className="flex items-end justify-between gap-3">
                      <Stat label="YOUR WALLET">
                        <span className="inline-flex items-center gap-1.5">
                          <span title={address}>{shortAddress(address)}</span>
                          <CopyIcon text={address} label="Copy wallet address" />
                        </span>
                      </Stat>
                      <TextButton onClick={logout}>Not you?</TextButton>
                    </div>

                    <SourcePicker options={options} value={source} onChange={setPicked} disabled={step !== "idle"} />

                    <Reveal show={balances !== null && !anyEnough}>
                      <Notice>
                        Your wallet doesn&apos;t hold enough yet. Send USDT on Whitechain, or USDC on{" "}
                        {customerChains.map((chain) => chainName(chain.id)).join(" or ")}, to{" "}
                        <CopyText text={address} />. This page updates by itself.
                        <span className="mt-2 block">
                          Testing? Get free test USDC from{" "}
                          <a
                            href="https://faucet.circle.com"
                            target="_blank"
                            rel="noreferrer"
                            className="text-foreground underline decoration-hair-strong underline-offset-4 hover:decoration-foreground"
                          >
                            Circle&apos;s faucet
                          </a>{" "}
                          (Base Sepolia or Ethereum Sepolia).
                        </span>
                      </Notice>
                    </Reveal>
                    <Reveal show={payment?.status === "failed"}>
                      <Problem>
                        The last try didn&apos;t go through
                        {payment?.failedReason ? `: ${payment.failedReason}` : ""}. Nothing was taken.
                      </Problem>
                    </Reveal>
                    <Reveal show={error !== null}>
                      <Problem>{error}</Problem>
                    </Reveal>

                    <PrimaryButton disabled={step !== "idle" || !chosenEnough} onClick={pay}>
                      {step !== "idle"
                        ? STEP_LABEL[step]
                        : chosen
                          ? `Pay ${trimAmount(formatUnits(chosen.cost, 6))} ${chosen.token}`
                          : "Pay"}
                    </PrimaryButton>
                    <FinePrint>
                      {oneClickNote(chosen?.source ?? null)}Expires in {formatCountdown(secondsLeft)}
                    </FinePrint>
                  </>
                )}
              </div>
            </Swap>
          </CardContent>
        </div>
      </Swap>
    </PayShell>
  );
}

function sameSource(a: Source | null, b: Source | null): boolean {
  return a !== null && b !== null && encodeSource(a) === encodeSource(b);
}

/** USDC base units as "6.03". */
function formatUsdc(units: bigint): string {
  return trimAmount(formatUnits(units, 6));
}

/** "One click · " for the cross-chain and split payments: what the customer does, before the countdown. */
function oneClickNote(source: Source | null): string {
  if (source?.kind === "usdc" || source?.kind === "split") return "One click · ";
  return "";
}

/**
 * A8 split: when no one chain holds enough USDC but two together do, take as much as possible from
 * the fuller chain (leaving the other at least the Order Desk's minimum) and the rest from the
 * other. `null` when one chain is enough, or two are not.
 */
function planSplit(cost: bigint, held: { chainId: number; balance: bigint | null }[]): SplitPart[] | null {
  const known = held
    .filter((entry): entry is { chainId: number; balance: bigint } => entry.balance !== null)
    .sort((a, b) => (b.balance > a.balance ? 1 : b.balance < a.balance ? -1 : 0));
  if (known.length < 2 || known.some((entry) => entry.balance >= cost)) return null;
  const [fuller, other] = known;
  const first = fuller.balance < cost - MIN_SPLIT_LEG ? fuller.balance : cost - MIN_SPLIT_LEG;
  const rest = cost - first;
  if (first < MIN_SPLIT_LEG || rest > other.balance) return null;
  return [
    { chainId: fuller.chainId, amount: first },
    { chainId: other.chainId, amount: rest },
  ];
}

function OnItsWay({ payment }: { payment: Payment | null }) {
  if (payment?.status === "failed") {
    return <Problem>This payment couldn&apos;t be sent{payment.failedReason ? `: ${payment.failedReason}` : ""}.</Problem>;
  }
  const text =
    payment?.status === "sent"
      ? "Waiting for Whitechain to confirm…"
      : payment?.status === "sending"
        ? "Sending your payment…"
        : "Getting your payment ready…";
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <Loader2 className="size-6 animate-spin text-muted-foreground" aria-hidden="true" />
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">{text}</p>
      {payment?.explorerUrl && <ExplorerLink href={payment.explorerUrl} />}
    </div>
  );
}

function Done({ order, amount }: { order: Order; amount: string }) {
  const embedded = useEmbedded();
  const legs = currentLegs(order);
  const crosschain = order.path === "crosschain" || order.path === "split";
  const settlement = crosschain ? order.claim?.explorerUrl : order.payment?.explorerUrl;
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <span className="grid size-12 place-items-center rounded-full bg-white text-black shadow-[0_0_0_8px_rgba(255,255,255,0.06)]">
        <Check className="size-5" aria-hidden="true" />
      </span>
      <p className="text-2xl font-normal tracking-[-0.02em] embedded:text-xl">Payment confirmed</p>
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
        {order.merchant.name} received {amount} USDT on Whitechain.
        {legs.length > 0 && crosschain ? ` You paid ${describeLegs(legs)}.` : ""}
      </p>
      <div className="flex flex-wrap justify-center gap-x-5 gap-y-2">
        {settlement && <ExplorerLink href={settlement} />}
        <TextButton asChild>
          <a href={`/receipt/${order.orderId}`}>Receipt</a>
        </TextButton>
        {embedded && (
          <TextButton onClick={() => window.parent.postMessage({ type: "uniform:close" }, "*")}>Close</TextButton>
        )}
      </div>
    </div>
  );
}

function Returned({ order }: { order: Order }) {
  const legs = currentLegs(order);
  // In a split, a part whose lock never landed was never taken, so only the locked parts came back.
  const locked = legs.filter((leg) => leg.lockTx);
  const outcome =
    legs.length === 0
      ? "your payment went back to you."
      : locked.length === 0
        ? "nothing left your wallet."
        : `your ${describeLegs(locked)} went back to your wallet.` +
          (locked.length < legs.length ? " The rest never left it." : "");
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <p className="text-2xl font-normal tracking-[-0.02em] embedded:text-xl">Your money is back</p>
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
        The merchant couldn&apos;t be paid in time, so {outcome}
      </p>
      <div className="flex flex-wrap justify-center gap-x-5 gap-y-1">
        {legs
          .filter((leg) => leg.refundTx)
          .map((leg) => (
            <ExplorerLink
              key={leg.lockId}
              href={explorerTx(leg.chainId, leg.refundTx as string)}
              label={legs.length > 1 ? chainName(leg.chainId) : undefined}
            />
          ))}
      </div>
    </div>
  );
}

/** The widget card's header and body: merchant and order, the amount, and what the customer should know. */
function CompactSummary({
  merchant,
  reference,
  amount,
  secondsLeft,
}: {
  merchant: string;
  reference: string;
  amount: string;
  secondsLeft: number;
}) {
  const payWith = customerChains.length > 0 ? "USDT or USDC" : "USDT";
  return (
    <div className="grid gap-4 px-(--card-spacing)">
      <div className="flex items-baseline justify-between gap-3">
        <h1 className="truncate text-[15px] font-medium">{merchant}</h1>
        <span className="font-mono text-[10px] tracking-[0.08em] text-faint uppercase">{reference}</span>
      </div>
      <p className="flex items-baseline gap-2">
        <span className="font-display text-[32px] leading-none tracking-[-0.03em]">{amount}</span>
        <span className="font-mono text-[11px] text-muted-foreground">USDT</span>
      </p>
      <dl className="grid gap-2 text-[12.5px]">
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Pay with</dt>
          <dd>{payWith}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Network fee</dt>
          <dd>None, we cover it</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Time to pay</dt>
          <dd className="tabular-nums">{secondsLeft > 0 ? formatCountdown(secondsLeft) : "Expired"}</dd>
        </div>
      </dl>
    </div>
  );
}

/** How long the wallet may take before the page says what is missing and offers a way out. */
const WALLET_WAIT_MS = 8000;

/**
 * After login, until Privy hands us the customer's wallet. Says "creating" only when the account
 * has no wallet yet; if it takes too long, shows what is missing and offers to retry or, inside
 * the widget's card, to open the pay page in its own tab.
 */
function WalletWait({
  accountWallet,
  walletsReady,
  embedded,
}: {
  accountWallet: string | null;
  walletsReady: boolean;
  embedded: boolean;
}) {
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setSlow(true), WALLET_WAIT_MS);
    return () => clearTimeout(timer);
  }, []);

  const text = accountWallet ? "Opening your wallet…" : "Creating your wallet…";
  if (!slow) {
    return (
      <p className="flex items-center gap-2 text-[14.5px] leading-[1.7] text-muted-foreground embedded:text-[13px]">
        <Loader2 className="size-3.5 animate-spin" /> {text}
      </p>
    );
  }
  return (
    <div className="grid gap-3">
      <Notice>
        Your wallet is taking longer than it should.{" "}
        {accountWallet
          ? `Your account has wallet ${shortAddress(accountWallet as Address)}, but it hasn't opened in this ${embedded ? "window" : "page"} yet.`
          : "Your account has no wallet yet, and creating one hasn't finished."}{" "}
        {walletsReady ? "" : "The wallet service is still loading."}
      </Notice>
      <PrimaryButton onClick={() => window.location.reload()}>Try again</PrimaryButton>
      {embedded && (
        <TextButton onClick={() => window.open(window.location.href, "_blank", "noopener")}>
          Open in a new tab instead
        </TextButton>
      )}
    </div>
  );
}

/** A cross-chain note may only move this wallet's USDC into our SourceLock on its own chain. */
function noteFits(note: Quote["toSign"][number], chainId: number, address: Address): boolean {
  const chain = customerChain(chainId);
  if (!chain) return false;
  const message = note.typedData.message;
  return (
    note.chainId === chainId &&
    Number(note.typedData.domain.chainId) === chainId &&
    note.typedData.domain.verifyingContract.toLowerCase() === chain.usdc.toLowerCase() &&
    message.to?.toLowerCase() === chain.sourceLock.toLowerCase() &&
    message.from?.toLowerCase() === address.toLowerCase() &&
    message.nonce === note.lockId
  );
}

/** Every way to pay this order, with what it costs and what the wallet holds there. */
function paymentOptions(units: bigint | null, balances: Balances | null, address: Address | null): SourceOption[] {
  if (units === null) return [];
  const mine = balances && balances.owner === address ? balances.units : {};
  const usdt: SourceOption = {
    source: { kind: "usdt" },
    token: "USDT",
    chain: chainName(shopChain.id),
    cost: units,
    balance: mine.usdt ?? null,
  };
  const usdc = customerChains.map(
    (chain): SourceOption => ({
      source: { kind: "usdc", chainId: chain.id },
      token: "USDC",
      chain: chainName(chain.id),
      cost: withProviderFee(units),
      balance: mine[`usdc:${chain.id}`] ?? null,
      note: "incl. 0.3% provider fee",
    }),
  );
  const parts = planSplit(
    withProviderFee(units),
    customerChains.map((chain) => ({ chainId: chain.id, balance: mine[`usdc:${chain.id}`] ?? null })),
  );
  const split: SourceOption[] = parts
    ? [
        {
          source: { kind: "split", parts },
          token: "USDC",
          chain: parts.map((part) => chainName(part.chainId)).join(" + "),
          cost: withProviderFee(units),
          balance: parts.reduce((sum, part) => sum + (mine[`usdc:${part.chainId}`] ?? BigInt(0)), BigInt(0)),
          note: "incl. 0.3% provider fee",
          detail: parts.map((part) => `${formatUsdc(part.amount)} on ${chainName(part.chainId)}`).join(" + "),
        },
      ]
    : [];
  return [usdt, ...usdc, ...split];
}
