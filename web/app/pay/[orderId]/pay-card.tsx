"use client";

// The pay page. The customer logs in with email (Privy gives them a wallet), picks what to pay
// with, and signs. They never pay gas: the Relayer sends every transaction.
//
// - USDT on Whitechain (Path A): one signed note; the Relayer sends it to DirectPay.
// - USDC on a customer chain (cross-chain): one signature makes the payment's key (the secret,
//   kept only in this page's memory), a second signs the USDC note that locks it for the
//   provider. Once the provider has locked USDT for the merchant, this page checks that lock on
//   chain itself and only then hands over the key, which pays the merchant.
// - A priced token (WETH, WBTC) on a customer chain: the same, but the note is a Calibur batch
//   (approve, then lock) signed by the wallet itself, with an EIP-7702 authorization to Calibur
//   the first time. Both are signed silently behind the one Pay click; the Gas Payer sends them.

import { useCreateWallet, usePrivy, useSign7702Authorization, useSignTypedData, useWallets } from "@privy-io/react-auth";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { decodeFunctionData, erc20Abi, formatUnits, parseUnits, zeroAddress, type Address, type Hex } from "viem";
import { Check, Loader2 } from "lucide-react";
import {
  chainName,
  checkShopLock,
  customerChain,
  customerChains,
  estimateTokenUnits,
  explorerTx,
  formatTokenAmount,
  providerFeeRange,
  shopChain,
  shortAddress,
  tokenBalance,
  trimAmount,
  usdPrices,
  usdcBalance,
  usdtBalance,
  walletCode,
  walletNonce,
  withProviderFee,
  type CustomerChain,
  type PricedToken,
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
  type Authorization,
  type BatchQuote,
  type BatchTypedData,
  type Order,
  type Quote,
  type Payment,
  type TypedData,
} from "@/lib/desk";
import { secretFromSignature, secretTypedData } from "@/lib/secret";
import { useEmbedded } from "@/app/_ui/embedded";
import { EmailLogin } from "./email-login";
import { ScanToPay } from "./scan-to-pay";
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
  LoadingBody,
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
/** How often to look at an order not paid yet: it may be paid from a phone that scanned its QR. */
const WAITING_POLL_MS = 4_000;
/** Orders still waiting for a payment to start. */
const WAITING = new Set(["created", "quoted"]);
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
/** A way to pay the wallet holds enough for, and that can be used right now. */
const usable = (option: SourceOption) =>
  option.unavailable === undefined && option.balance !== null && option.balance >= option.cost;
/** Providers set their own fee: the order says the cheapest one now, else the highest allowed. */
const feeNote = (feeBps: number | null) =>
  feeBps === null ? `incl. provider fee, at most ${providerFeeRange.max / 100}%` : `incl. ${feeBps / 100}% provider fee`;

/** How often to refresh the market prices behind the priced tokens' estimates. */
const PRICE_POLL_MS = 60_000;
/** CoinGecko ids of every priced token on the customer chains. */
const PRICE_IDS = [...new Set(customerChains.flatMap((chain) => chain.tokens.map((token) => token.priceId)))];
/** Every priced token, with its chain: one balance and one pay option each. */
const PRICED = customerChains.flatMap((chain) => chain.tokens.map((token) => ({ chain, token })));
const tokenKey = (chainId: number, symbol: string) => `token:${chainId}:${symbol}`;
const DIDNT_CHECK_OUT = "The payment details didn't check out. Nothing was signed.";

/** The payment's key, in memory only. */
type Key = { orderId: string; secret: Hex; hashlock: Hex };

/** Balances by source, read for one wallet. `null` while unknown. */
type Balances = { owner: Address; units: Record<string, bigint | null> };

export function PayCard({ orderId }: { orderId: string }) {
  const embedded = useEmbedded();
  const { ready, authenticated, logout, user } = usePrivy();
  const { wallets, ready: walletsReady } = useWallets();
  const { signTypedData } = useSignTypedData();
  const { signAuthorization } = useSign7702Authorization();

  const [order, setOrder] = useState<Order | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [balances, setBalances] = useState<Balances | null>(null);
  /** USD per token by CoinGecko id, for estimates only. */
  const [prices, setPrices] = useState<Record<string, number>>({});
  const [picked, setPicked] = useState<Source | null>(null);
  const [step, setStep] = useState<Step>("idle");
  const [error, setError] = useState<string | null>(null);
  const [key, setKey] = useState<Key | null>(null);
  const [lockProblem, setLockProblem] = useState<string | null>(null);
  const revealing = useRef(false);
  const now = useNow();

  const wallet = wallets.find((candidate) => candidate.walletClientType === "privy") ?? null;
  const address = (wallet?.address ?? null) as Address | null;

  // Privy makes a wallet at login, but never again if that was cut short (a closed tab, a reload
  // mid-login): a logged-in account with no wallet would wait forever. Make it here, once.
  const { createWallet } = useCreateWallet();
  const creating = useRef(false);
  const hasWallet = user?.wallet !== undefined;
  useEffect(() => {
    if (!ready || !authenticated || !user || hasWallet || creating.current) return;
    creating.current = true;
    createWallet().catch((problem: unknown) => {
      creating.current = false;
      console.error("creating the wallet failed", problem);
    });
  }, [ready, authenticated, user, hasWallet, createWallet]);
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

  // While the payment moves, follow it. While it waits, look now and then: it may be paid from a
  // phone that scanned this order's QR.
  const moving = order ? MOVING.has(order.status) : false;
  const waiting = order ? WAITING.has(order.status) : false;
  useEffect(() => {
    if (!moving && !waiting) return;
    const timer = setInterval(refresh, moving ? ORDER_POLL_MS : WAITING_POLL_MS);
    return () => clearInterval(timer);
  }, [moving, waiting, refresh]);

  // Every way to pay, with what it costs and whether the wallet has it.
  const feeBps = order?.providerFeeBps ?? null;
  const options = useMemo(
    () => paymentOptions(units, balances, address, prices, feeBps),
    [units, balances, address, prices, feeBps],
  );

  // Market prices for the priced tokens' estimates. A failed read keeps the last prices.
  useEffect(() => {
    if (PRICE_IDS.length === 0) return;
    let current = true;
    const load = () =>
      usdPrices(PRICE_IDS).then(
        (loaded) => current && setPrices((previous) => ({ ...previous, ...loaded })),
        () => undefined,
      );
    void load();
    const timer = setInterval(load, PRICE_POLL_MS);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, []);

  const anyEnough = options.some((option) => usable(option));
  // Paying from another chain needs a provider who can cover this order now; the order says.
  const noProvider = order !== null && options.length > 0 && (order.providerFeeBps ?? null) === null;
  const firstEnough = options.find((option) => usable(option));
  // The picked row as built from the latest balances: a split's parts follow them.
  const pickedNow = picked ? (options.find((option) => sameSource(option.source, picked))?.source ?? null) : null;
  const source = pickedNow ?? firstEnough?.source ?? options[0]?.source ?? null;
  const chosen = options.find((option) => sameSource(option.source, source)) ?? null;
  const chosenEnough = chosen !== null && usable(chosen);

  const readBalances = useCallback(async () => {
    if (!address) return;
    const reads: [string, Promise<bigint>][] = [
      ["usdt", usdtBalance(address)],
      ...customerChains.map((chain): [string, Promise<bigint>] => [`usdc:${chain.id}`, usdcBalance(chain, address)]),
      ...PRICED.map(({ chain, token }): [string, Promise<bigint>] => [
        tokenKey(chain.id, token.symbol),
        tokenBalance(chain.id, token.address, address),
      ]),
    ];
    // Shown together once every chain has answered (one batched request per chain); a failed read
    // keeps the last value.
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
  async function signNote(typedData: TypedData | BatchTypedData, title: string, description: string): Promise<string> {
    if (!address) throw new Error("no wallet");
    // A Calibur batch's domain has a `salt` (bytes32 hex), passed through as is: Privy types it as
    // an ArrayBuffer, but the wallet takes eth_signTypedData_v4 JSON, where bytes32 is hex text.
    const data = {
      ...typedData,
      domain: { ...typedData.domain, chainId: Number(typedData.domain.chainId) },
    } as unknown as Parameters<typeof signTypedData>[0];
    const options = {
      address,
      uiOptions: { showWalletUIs: false, title, description, buttonText: "Sign and pay" },
    };
    try {
      return (await signTypedData(data, options)).signature;
    } catch (problem) {
      // Some wallets sign typed data only for the chain they are on.
      if (!wallet || !/chain/i.test(String(problem))) throw problem;
      await wallet.switchChain(Number(typedData.domain.chainId));
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

  /**
   * A priced token (WETH, WBTC): one Calibur batch, approve then lock, signed by the wallet itself.
   * The first time on a chain the wallet also signs an EIP-7702 authorization to Calibur. Both are
   * silent: the Pay click is the confirmation, and our Gas Payer sends the transaction.
   */
  async function payToken(chainId: number, symbol: string) {
    const chain = customerChain(chainId);
    const token = chain?.tokens.find((candidate) => candidate.symbol === symbol);
    if (!order || !address || !chain || !token || !chain.calibur) return;
    const calibur = chain.calibur;
    setStep("keying");
    const made = keyFits && key ? key : await makeKey();
    setStep("quoting");
    const quote = await getQuote(orderId, address, { hashlock: made.hashlock, chainId, token: symbol });
    const held = balances?.owner === address ? (balances.units[tokenKey(chainId, symbol)] ?? null) : null;
    // Read the balance afresh: the one shown may be a few seconds old.
    const balance = await tokenBalance(chainId, token.address, address).catch(() => held);
    const batch = quote.toSign[0];
    // The most this page will sign for: its own estimate at the price it shows, plus 5% for the
    // price moving in between. Without a price it signs nothing.
    const price = prices[token.priceId];
    const ceiling = price
      ? (estimateTokenUnits(withProviderFee(parseUnits(order.amount, 6), order.providerFeeBps ?? undefined), price, token.decimals) *
          BigInt(105)) /
        BigInt(100)
      : BigInt(0);
    const expected = { orderId: order.orderId, hashlock: made.hashlock, ceiling };
    if (quote.toSign.length !== 1 || !batch || !batchFits(batch, chain, token, address, balance, expected)) throw new Error(DIDNT_CHECK_OUT);

    // Delegated already? Then the wallet's code is `0xef0100 ‖ calibur` and no authorization is needed.
    // A wallet set up with another smart account is left alone: signing would replace it.
    const code = (await walletCode(chainId, address)).toLowerCase();
    const caliburCode = `0xef0100${calibur.slice(2).toLowerCase()}`;
    if (code !== "0x" && code !== caliburCode) {
      throw new Error(`This wallet already runs another smart account on ${chainName(chainId)}. Pay with USDC instead.`);
    }
    let authorization: Authorization | undefined;
    setStep("signing");
    if (code !== caliburCode) {
      const nonce = await walletNonce(chainId, address);
      const signed = await signAuthorization({ contractAddress: calibur, chainId, nonce }, { address });
      if (signed.address.toLowerCase() !== calibur.toLowerCase() || Number(signed.chainId) !== chainId) {
        throw new Error(DIDNT_CHECK_OUT);
      }
      authorization = {
        chainId,
        address: signed.address,
        nonce: Number(signed.nonce),
        yParity: Number(signed.yParity),
        r: signed.r,
        s: signed.s,
      };
    }
    const signature = await signNote(
      batch.typedData,
      `Pay ${formatTokenAmount(parseUnits(batch.amount, batch.decimals), batch.decimals)} ${symbol}`,
      `From ${chainName(chainId)}. Held for the provider until ${order.merchant.name} is paid; it comes back to you if not.`,
    );
    setStep("submitting");
    await submitLegs(orderId, quote.quoteId, [signature], authorization);
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
      else if (source.kind === "token") await payToken(source.chainId, source.symbol);
      else await paySplit(source.parts);
    } catch (problem) {
      // The customer sees a short message; the full error goes to the console for support.
      console.error("pay failed", problem);
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
  const paidHere = address !== null && order.payer?.toLowerCase() === address.toLowerCase();
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
                    {/* Only the wallet that paid holds the key: a screen that showed the QR waits for the phone. */}
                    <Reveal show={order.status === "destination_locked" && !keyFits && !claimLive && paidHere}>
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
                {view === "log-in" && (
                  <>
                    <EmailLogin />
                    <ScanToPay orderId={order.orderId} />
                  </>
                )}
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

                    {balances?.owner !== address ? (
                      <LoadingBody text="Reading your balances…" />
                    ) : (
                      <>
                    <SourcePicker
                      options={options}
                      value={source}
                      onChange={setPicked}
                      disabled={step !== "idle"}
                      feeNote={
                        feeBps === null
                          ? `the provider's fee, at most ${providerFeeRange.max / 100}%`
                          : `the provider's ${feeBps / 100}% fee`
                      }
                    />

                    <Reveal show={noProvider}>
                      <Notice>
                        Paying from another chain isn&apos;t available for this amount right now: no provider has enough
                        USDT free. Pay with USDT on Whitechain, or try again in a few minutes.
                      </Notice>
                    </Reveal>
                    <Reveal show={balances !== null && !anyEnough && !noProvider}>
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
                          ? `Pay ${chosen.approx ? "≈" : ""}${formatTokenAmount(chosen.cost, chosen.decimals)} ${chosen.token}`
                          : "Pay"}
                    </PrimaryButton>
                    <FinePrint>
                      {oneClickNote(chosen?.source ?? null)}Expires in {formatCountdown(secondsLeft)}
                    </FinePrint>
                      </>
                    )}
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
  if (source?.kind === "usdc" || source?.kind === "split" || source?.kind === "token") return "One click · ";
  return "";
}

/**
 * Split: whenever USDC on two chains together covers the order, a way to pay from both. It uses up
 * the smaller balance first (the leftovers that are the reason to split), leaving the fuller chain
 * at least the Order Desk's minimum leg, and takes the rest from the fuller one. Offered even when
 * one chain alone is enough, as a choice next to it. `null` when the two together are not enough.
 */
function planSplit(cost: bigint, held: { chainId: number; balance: bigint | null }[]): SplitPart[] | null {
  const known = held
    .filter((entry): entry is { chainId: number; balance: bigint } => entry.balance !== null && entry.balance >= MIN_SPLIT_LEG)
    .sort((a, b) => (a.balance > b.balance ? 1 : a.balance < b.balance ? -1 : 0));
  if (known.length < 2) return null;
  const [smaller, fuller] = known;
  const first = smaller.balance < cost - MIN_SPLIT_LEG ? smaller.balance : cost - MIN_SPLIT_LEG;
  const rest = cost - first;
  if (first < MIN_SPLIT_LEG || rest > fuller.balance) return null;
  return [
    { chainId: smaller.chainId, amount: first },
    { chainId: fuller.chainId, amount: rest },
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
  const extra = [...new Set(PRICED.map(({ token }) => token.symbol))];
  const tokens = ["USDT", ...(customerChains.length > 0 ? ["USDC"] : []), ...extra];
  const payWith = tokens.length > 1 ? `${tokens.slice(0, -1).join(", ")} or ${tokens[tokens.length - 1]}` : "USDT";
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

/**
 * A priced token's batch may only approve exactly the quoted amount of this token to our SourceLock
 * on this chain, then call that SourceLock, from this wallet (the domain's verifyingContract), under
 * Calibur's domain, within what the wallet holds.
 */
function batchFits(
  batch: BatchQuote["toSign"][number],
  chain: CustomerChain,
  token: PricedToken,
  address: Address,
  balance: bigint | null,
  expected: { orderId: string; hashlock: string; ceiling: bigint },
): boolean {
  if (!chain.calibur) return false;
  const same = (a: string | undefined, b: string) => typeof a === "string" && a.toLowerCase() === b.toLowerCase();
  const { domain, message } = batch.typedData;
  const calls = message?.batchedCall?.calls ?? [];
  let amount: bigint;
  try {
    amount = parseUnits(batch.amount, batch.decimals);
  } catch {
    return false;
  }
  if (
    batch.signing !== "batch" ||
    batch.chainId !== chain.id ||
    Number(domain.chainId) !== chain.id ||
    !same(batch.tokenAddress, token.address) ||
    batch.decimals !== token.decimals ||
    !same(batch.calibur, chain.calibur) ||
    !same(domain.verifyingContract, address) ||
    !same(domain.salt, `0x${"0".repeat(24)}${chain.calibur.slice(2)}`) ||
    calls.length !== 2 ||
    !same(calls[0].to, token.address) ||
    !same(calls[1].to, chain.sourceLock) ||
    calls.some((call) => BigInt(call.value || "0") !== BigInt(0)) ||
    !same(message.executor, zeroAddress) ||
    message.batchedCall?.revertOnFailure !== true ||
    BigInt(message.keyHash ?? "1") !== BigInt(0) ||
    amount <= BigInt(0) ||
    amount > expected.ceiling ||
    balance === null ||
    amount > balance
  ) {
    return false;
  }
  try {
    const approve = decodeFunctionData({ abi: erc20Abi, data: calls[0].data as Hex });
    const lock = decodeFunctionData({ abi: SOURCE_LOCK_LOCK_ABI, data: calls[1].data as Hex });
    const [params, lockToken, lockAmount] = lock.args;
    return (
      approve.functionName === "approve" &&
      same(approve.args[0], chain.sourceLock) &&
      approve.args[1] === amount &&
      same(params.orderId, expected.orderId) &&
      same(params.hashlock, expected.hashlock) &&
      same(params.outToken, zeroAddress) &&
      same(lockToken, token.address) &&
      lockAmount === amount
    );
  } catch {
    return false;
  }
}

/** `SourceLock.lock(params, token, amount)`, to read what a batch locks before signing it. */
const SOURCE_LOCK_LOCK_ABI = [
  {
    type: "function",
    name: "lock",
    stateMutability: "nonpayable",
    inputs: [
      {
        name: "p",
        type: "tuple",
        components: [
          { name: "orderId", type: "bytes32" },
          { name: "receiver", type: "address" },
          { name: "minUsdcOut", type: "uint256" },
          { name: "hashlock", type: "bytes32" },
          { name: "expiry", type: "uint64" },
          { name: "outToken", type: "address" },
        ],
      },
      { name: "token", type: "address" },
      { name: "amount", type: "uint256" },
    ],
    outputs: [{ name: "lockId", type: "bytes32" }],
  },
] as const;

/** Every way to pay this order, with what it costs and what the wallet holds there. */
function paymentOptions(
  units: bigint | null,
  balances: Balances | null,
  address: Address | null,
  prices: Record<string, number>,
  feeBps: number | null,
): SourceOption[] {
  if (units === null) return [];
  const withFee = (amount: bigint) => withProviderFee(amount, feeBps ?? undefined);
  // No provider can take the order from another chain right now: those ways are shown, not offered.
  const unavailable = feeBps === null ? "no provider right now" : undefined;
  const mine = balances && balances.owner === address ? balances.units : {};
  const usdt: SourceOption = {
    source: { kind: "usdt" },
    token: "USDT",
    chain: chainName(shopChain.id),
    cost: units,
    decimals: shopChain.usdtDecimals,
    balance: mine.usdt ?? null,
  };
  const usdc = customerChains.map(
    (chain): SourceOption => ({
      source: { kind: "usdc", chainId: chain.id },
      token: "USDC",
      chain: chainName(chain.id),
      cost: withFee(units),
      decimals: chain.usdcDecimals,
      balance: mine[`usdc:${chain.id}`] ?? null,
      note: feeNote(feeBps),
      unavailable,
    }),
  );
  const parts = planSplit(
    withFee(units),
    customerChains.map((chain) => ({ chainId: chain.id, balance: mine[`usdc:${chain.id}`] ?? null })),
  );
  const split: SourceOption[] = parts
    ? [
        {
          source: { kind: "split", parts },
          token: "USDC",
          chain: parts.map((part) => chainName(part.chainId)).join(" + "),
          cost: withFee(units),
          decimals: 6,
          balance: parts.reduce((sum, part) => sum + (mine[`usdc:${part.chainId}`] ?? BigInt(0)), BigInt(0)),
          note: feeNote(feeBps),
          unavailable,
          detail: parts.map((part) => `${formatUsdc(part.amount)} on ${chainName(part.chainId)}`).join(" + "),
        },
      ]
    : [];
  // Priced tokens the wallet holds, once their price is known: the cost is an estimate.
  const priced = PRICED.flatMap(({ chain, token }): SourceOption[] => {
    const balance = mine[tokenKey(chain.id, token.symbol)] ?? null;
    const price = prices[token.priceId];
    if (balance === null || balance <= BigInt(0) || !price) return [];
    return [
      {
        source: { kind: "token", chainId: chain.id, symbol: token.symbol },
        token: token.symbol,
        chain: chainName(chain.id),
        cost: estimateTokenUnits(withFee(units), price, token.decimals),
        decimals: token.decimals,
        approx: true,
        unavailable,
        balance,
      },
    ];
  });
  return [usdt, ...usdc, ...split, ...priced];
}
