"use client";

// The pay page. The customer logs in with email (Privy gives them a wallet), picks what to pay
// with, and signs. They never pay gas: the Relayer sends every transaction.
//
// - USDT on Whitechain (Path A): one signed note; the Relayer sends it to DirectPay.
// - USDC on a customer chain (cross-chain): one signature makes the payment's key (the secret,
//   kept only in this page's memory), a second signs the USDC note that locks it for the
//   provider. Once the provider has locked USDT for the shop, this page checks that lock on
//   chain itself and only then hands over the key, which pays the shop.

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
  explain,
  getOrder,
  getQuote,
  revealSecret,
  submitDirect,
  submitLegs,
  type Order,
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
import { SourcePicker, type Source, type SourceOption } from "./source-picker";
import { Progress } from "./progress";

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

/** The payment's key, in memory only. */
type Key = { orderId: string; secret: Hex; hashlock: Hex };

/** Balances by source, read for one wallet. `null` while unknown. */
type Balances = { owner: Address; units: Record<string, bigint | null> };

export function PayCard({ orderId }: { orderId: string }) {
  const embedded = useEmbedded();
  const { ready, authenticated, login, logout, user } = usePrivy();
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
  const options: SourceOption[] = useMemo(() => {
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
    return [usdt, ...usdc];
  }, [units, balances, address]);

  const anyEnough = options.some((option) => option.balance !== null && option.balance >= option.cost);
  const firstEnough = options.find((option) => option.balance !== null && option.balance >= option.cost);
  const source = picked ?? firstEnough?.source ?? options[0]?.source ?? null;
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

  // Tell the shop's page once the order is paid: the widget shows us in a card (an iframe); an
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
      uiOptions: {
        // In the widget's card our own button is the confirmation: no Privy window over the card.
        showWalletUIs: !embedded,
        title: "Confirm this payment",
        description: "This signature makes a one-time key for this payment. It moves no money and costs nothing.",
        buttonText: "Confirm",
      },
    });
    const made = { orderId: order.orderId, ...secretFromSignature(signature as Hex) };
    setKey(made);
    return made;
  }, [address, units, order, signTypedData, embedded]);

  // Cross-chain: once the provider has locked USDT for the shop, check that lock on chain and
  // hand over the key. Runs again on every poll until the claim is under way.
  const leg = order?.legs[order.legs.length - 1] ?? null;
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
          merchant: order.shop.address as Address,
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

  async function signNote(typedData: TypedData, title: string, description: string): Promise<string> {
    if (!address) throw new Error("no wallet");
    const data = { ...typedData, domain: { ...typedData.domain, chainId: Number(typedData.domain.chainId) } };
    const options = { address, uiOptions: { showWalletUIs: !embedded, title, description, buttonText: "Sign and pay" } };
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
      `To ${order.shop.name}. No gas needed: Uniform pays the network fee.`,
    );
    setStep("submitting");
    await submitDirect(orderId, quote.quoteId, signature);
  }

  async function payCrossChain(chainId: number) {
    const chain = customerChain(chainId);
    if (!order || !address || !chain) return;
    setStep("keying");
    const made = keyFits && key ? key : await makeKey();
    setStep("quoting");
    const quote = await getQuote(orderId, address, { hashlock: made.hashlock, chainId });
    const note = quote.toSign[0];
    // The note may only move this wallet's USDC into our SourceLock on that chain.
    const message = note.typedData.message;
    const fits =
      note.chainId === chainId &&
      note.typedData.domain.verifyingContract.toLowerCase() === chain.usdc.toLowerCase() &&
      message.to?.toLowerCase() === chain.sourceLock.toLowerCase() &&
      message.from?.toLowerCase() === address.toLowerCase() &&
      message.nonce === note.lockId;
    if (!fits) throw new Error("The payment details didn't check out. Nothing was signed.");
    setStep("signing");
    const signature = await signNote(
      note.typedData,
      `Pay ${trimAmount(quote.total)} USDC`,
      `From ${chainName(chainId)}. Held for the provider until ${order.shop.name} is paid; it comes back to you if not.`,
    );
    setStep("submitting");
    await submitLegs(orderId, quote.quoteId, signature);
  }

  async function pay() {
    if (!source) return;
    setError(null);
    try {
      if (source.kind === "usdt") await payDirect();
      else await payCrossChain(source.chainId);
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
            <CompactSummary shop={order.shop.name} reference={order.reference} amount={amount} secondsLeft={secondsLeft} />
          ) : (
            <CardHeader className="gap-4">
              <div className="flex justify-between gap-3 font-mono text-[10.5px] tracking-[0.08em] text-label">
                <span>PAY</span>
                <span>ORDER {order.reference}</span>
              </div>
              <Separator className="bg-hair" />
              <h1 className="text-2xl font-normal tracking-[-0.02em]">{order.shop.name}</h1>
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
                  The shop is paid on Whitechain
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
                          The shop&apos;s USDT is locked and waiting. Confirm once more to release it: this page
                          doesn&apos;t keep your payment key.
                        </Notice>
                        <PrimaryButton onClick={finish}>Finish payment</PrimaryButton>
                      </div>
                    </Reveal>
                    <Reveal show={lockProblem !== null && order.status === "destination_locked"}>
                      <FinePrint>Checking the shop&apos;s lock: {lockProblem}</FinePrint>
                    </Reveal>
                    <Reveal show={error !== null}>
                      <Problem>{error}</Problem>
                    </Reveal>
                  </>
                )}
                {view === "expired" && (
                  <Notice>The time to pay this order has run out. Ask the shop for a new link.</Notice>
                )}
                {view === "starting" && <PrimaryButton disabled>Loading…</PrimaryButton>}
                {view === "log-in" && embedded && <EmailLogin />}
                {view === "log-in" && !embedded && (
                  <>
                    <PrimaryButton onClick={login}>Log in with email to pay</PrimaryButton>
                    <FinePrint>We create a wallet for you. Nothing to install.</FinePrint>
                  </>
                )}
                {view === "creating-wallet" && (
                  <WalletWait accountWallet={user?.wallet?.address ?? null} walletsReady={walletsReady} embedded={embedded} />
                )}
                {view === "pay" && address && (
                  <>
                    <div className="flex items-end justify-between gap-3">
                      <Stat label="YOUR WALLET">
                        <span title={address}>{shortAddress(address)}</span>
                      </Stat>
                      <TextButton onClick={logout}>Not you?</TextButton>
                    </div>

                    <SourcePicker options={options} value={source} onChange={setPicked} disabled={step !== "idle"} />

                    <Reveal show={balances !== null && !anyEnough}>
                      <Notice>
                        Your wallet doesn&apos;t hold enough yet. Send USDT on Whitechain, or USDC on{" "}
                        {customerChains.map((chain) => chainName(chain.id)).join(" or ")}, to{" "}
                        <CopyText text={address} />. This page updates by itself.
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
                      {chosen?.source.kind === "usdc" ? "Two signatures · " : ""}Expires in {formatCountdown(secondsLeft)}
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
  if (!a || !b) return false;
  if (a.kind === "usdt" || b.kind === "usdt") return a.kind === b.kind;
  return a.chainId === b.chainId;
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
  const leg = order.legs[order.legs.length - 1];
  const settlement = order.path === "crosschain" ? order.claim?.explorerUrl : order.payment?.explorerUrl;
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <span className="grid size-12 place-items-center rounded-full bg-white text-black shadow-[0_0_0_8px_rgba(255,255,255,0.06)]">
        <Check className="size-5" aria-hidden="true" />
      </span>
      <p className="text-2xl font-normal tracking-[-0.02em] embedded:text-xl">Payment confirmed</p>
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
        {order.shop.name} received {amount} USDT on Whitechain.
        {leg && order.path === "crosschain"
          ? ` You paid ${trimAmount(formatUnits(BigInt(leg.amount), 6))} USDC on ${chainName(leg.chainId)}.`
          : ""}
      </p>
      <div className="flex flex-wrap justify-center gap-x-5 gap-y-2">
        {settlement && <ExplorerLink href={settlement} />}
        <TextButton asChild>
          <a href={`/receipt/${order.orderId}`}>Receipt</a>
        </TextButton>
      </div>
    </div>
  );
}

function Returned({ order }: { order: Order }) {
  const leg = order.legs[order.legs.length - 1];
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <p className="text-2xl font-normal tracking-[-0.02em] embedded:text-xl">Your money is back</p>
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
        The shop couldn&apos;t be paid in time, so{" "}
        {leg
          ? `your ${trimAmount(formatUnits(BigInt(leg.amount), 6))} USDC went back to your wallet on ${chainName(leg.chainId)}.`
          : "your payment went back to you."}
      </p>
      {leg?.refundTx && <ExplorerLink href={explorerTx(leg.chainId, leg.refundTx)} />}
    </div>
  );
}

/** The widget card's header and body: shop and order, the amount, and what the customer should know. */
function CompactSummary({
  shop,
  reference,
  amount,
  secondsLeft,
}: {
  shop: string;
  reference: string;
  amount: string;
  secondsLeft: number;
}) {
  const payWith = customerChains.length > 0 ? "USDT or USDC" : "USDT";
  return (
    <div className="grid gap-4 px-(--card-spacing)">
      <div className="flex items-baseline justify-between gap-3">
        <h1 className="truncate text-[15px] font-medium">{shop}</h1>
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
