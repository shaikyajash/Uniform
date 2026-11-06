"use client";

// Path A: the customer pays USDT on Whitechain. They sign one note (no gas); the Relayer sends it.

import { usePrivy, useSignTypedData, useWallets } from "@privy-io/react-auth";
import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { parseUnits, type Address } from "viem";
import { formatUsdt, shopChain, shortAddress, trimAmount, usdtBalance } from "@/lib/chain";
import { explain, getOrder, getQuote, submitDirect, type Order, type Payment } from "@/lib/desk";
import { ArrowUpRight, Check, CircleAlert, Loader2, TriangleAlert } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

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
  const confirmed = order.status === "paid" || payment?.status === "confirmed";
  const onItsWay = order.status === "paying" && !confirmed;
  const secondsLeft = order.expiresAt - now;
  const expired =
    order.status === "expired" || ((order.status === "created" || order.status === "quoted") && secondsLeft <= 0);
  const view = confirmed
    ? "done"
    : onItsWay
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
        <div className="flex flex-col gap-5">
          <CardHeader className="gap-4">
            <div className="flex justify-between gap-3 font-mono text-[10.5px] tracking-[0.08em] text-label">
              <span>PAY</span>
              <span>ORDER {order.reference}</span>
            </div>
            <Separator className="bg-hair" />
            <h1 className="text-2xl font-normal tracking-[-0.02em]">{order.shop.name}</h1>
          </CardHeader>

          <CardContent className="relative grid gap-4">
            <div className="grid gap-2">
              <p className="flex items-baseline gap-2.5">
                <span className="bg-gradient-to-b from-white via-zinc-300 to-zinc-400 bg-clip-text font-display text-[56px] leading-none tracking-[-0.035em] text-transparent">
                  {amount}
                </span>
                <span className="font-mono text-sm text-muted-foreground">USDT</span>
              </p>
              <p className="flex flex-wrap items-center gap-2 text-[14.5px] leading-[1.7] text-muted-foreground">
                on Whitechain
                <Badge
                  variant="outline"
                  className="border-hair-strong font-mono text-[10px] tracking-[0.08em] text-label"
                >
                  NO GAS NEEDED
                </Badge>
              </p>
            </div>

            <Swap id={view}>
              <div className="grid gap-4">
                {view === "done" && <Done order={order} amount={amount} />}
                {view === "on-its-way" && <OnItsWay payment={payment} />}
                {view === "expired" && (
                  <Notice>The time to pay this order has run out. Ask the shop for a new link.</Notice>
                )}
                {view === "starting" && <PrimaryButton disabled>Loading…</PrimaryButton>}
                {view === "log-in" && (
                  <>
                    <PrimaryButton onClick={login}>Log in with email to pay</PrimaryButton>
                    <FinePrint>We create a wallet for you. Nothing to install.</FinePrint>
                  </>
                )}
                {view === "creating-wallet" && (
                  <p className="text-[14.5px] leading-[1.7] text-muted-foreground">Creating your wallet…</p>
                )}
                {view === "pay" && address && (
                  <>
                    <div className="flex flex-wrap items-end justify-between gap-x-5 gap-y-2.5 rounded-lg border border-hair bg-muted px-3.5 py-3">
                      <Stat label="FROM">
                        <span title={address}>{shortAddress(address)}</span>
                      </Stat>
                      <Stat label="BALANCE">{myBalance === null ? "…" : `${formatUsdt(myBalance)} USDT`}</Stat>
                      <TextButton onClick={logout}>Not you?</TextButton>
                    </div>

                    <Reveal show={myBalance !== null && !enough}>
                      <Notice>
                        You need {amount} USDT on Whitechain. Send USDT to <CopyText text={address} /> and this page
                        updates by itself.
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

                    <PrimaryButton disabled={step !== "idle" || !enough} onClick={pay}>
                      {step === "idle" ? `Pay ${amount} USDT` : STEP_LABEL[step]}
                    </PrimaryButton>
                    <FinePrint>Expires in {formatCountdown(secondsLeft)}</FinePrint>
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

/* ---------- Motion, as on Garden: the card glides to its new height and its contents crossfade ---------- */

const GLIDE = { duration: 0.35, ease: [0.22, 1, 0.36, 1] } as const;

/** Animates its height to whatever its content measures, instead of snapping. */
function AutoHeight({ children }: { children: ReactNode }) {
  const inner = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | "auto">("auto");
  useEffect(() => {
    const element = inner.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setHeight(element.offsetHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return (
    <motion.div initial={false} animate={{ height }} transition={GLIDE} className="overflow-hidden">
      {/* `relative`: a crossfading screen is laid over the new one while it leaves. */}
      <div ref={inner} className="relative">
        {children}
      </div>
    </motion.div>
  );
}

/** Crossfades when `id` changes: the old content fades up and out while the new fades in. */
function Swap({ id, children }: { id: string; children: ReactNode }) {
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.div
        key={id}
        initial={{ opacity: 0, y: 6, filter: "blur(4px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        exit={{ opacity: 0, y: -6, filter: "blur(4px)" }}
        transition={{ duration: 0.25, ease: GLIDE.ease }}
      >
        {children}
      </motion.div>
    </AnimatePresence>
  );
}

/** Opens and closes a notice smoothly instead of popping it in. */
function Reveal({ show, children }: { show: boolean; children: ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={GLIDE}
          className="-my-2 overflow-hidden py-2"
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/**
 * The card while the order loads: the same shell, gaps and heights as the logged-out card
 * (bar 15px, title 32px, amount 56px, network line 24.6px, button 44px, small print 15px),
 * so nothing jumps when the real card arrives.
 */
export function PayCardSkeleton() {
  return (
    <PayShell busy>
      <SkeletonBody />
    </PayShell>
  );
}

function SkeletonBody() {
  return (
    <div className="flex flex-col gap-5">
      <CardHeader className="gap-4">
        <div className="flex justify-between">
          <Skeleton className="h-[15px] w-8" />
          <Skeleton className="h-[15px] w-28" />
        </div>
        <Separator className="bg-hair" />
        <Skeleton className="h-8 w-40" />
      </CardHeader>
      <CardContent className="grid gap-4">
        <div className="grid gap-2">
          <Skeleton className="h-14 w-36" />
          <Skeleton className="h-[24.6px] w-56" />
        </div>
        <Skeleton className="h-11 w-full rounded-full" />
        <Skeleton className="mx-auto h-[15px] w-64" />
      </CardContent>
    </div>
  );
}

/* ---------- Building blocks, styled like the home page ---------- */

/** The dark panel from the home page's feature blocks; its height glides between screens. */
function PayShell({ children, busy = false }: { children: ReactNode; busy?: boolean }) {
  return (
    <MotionConfig reducedMotion="user">
      <Card
        aria-busy={busy}
        className="w-full max-w-[420px] gap-0 rounded-2xl bg-gradient-to-b from-panel-2 to-panel py-6 ring-hair-strong shadow-[inset_0_1px_0_rgba(255,255,255,0.12),0_40px_80px_-40px_#000] [--card-spacing:--spacing(6)]"
      >
        <AutoHeight>{children}</AutoHeight>
      </Card>
    </MotionConfig>
  );
}

/** The home page's white pill, with its uppercase system-font text. */
function PrimaryButton(props: React.ComponentProps<typeof Button>) {
  return (
    <Button
      className="h-11 w-full rounded-full border-white/30 font-system text-sm font-medium uppercase hover:bg-white/90 disabled:opacity-35"
      {...props}
    />
  );
}

/** Small mono uppercase link, like the panel bars on the home page. */
function TextButton(props: React.ComponentProps<typeof Button>) {
  return (
    <Button
      variant="link"
      className="h-auto p-0 font-mono text-[10.5px] tracking-[0.08em] text-label uppercase underline decoration-faint underline-offset-[3px] hover:decoration-foreground"
      {...props}
    />
  );
}

/** A label over a value, like the stats row under the home page's cross-chain example. */
function Stat({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid gap-1">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">{label}</small>
      <b className="font-mono text-[15px] font-normal text-[#d9d9de]">{children}</b>
    </div>
  );
}

function FinePrint({ children }: { children: ReactNode }) {
  return (
    <p className="text-center font-mono text-[10.5px] tracking-[0.08em] text-faint uppercase">{children}</p>
  );
}

/** Something the customer can fix (not enough USDT, the time ran out). */
function Notice({ children }: { children: ReactNode }) {
  return (
    <Alert className="border-hair border-l-2 border-l-warn bg-muted px-3.5 py-3">
      <TriangleAlert className="text-warn" />
      <AlertDescription className="text-[13.5px] leading-[1.6] text-label">{children}</AlertDescription>
    </Alert>
  );
}

/** Something went wrong. */
function Problem({ children }: { children: ReactNode }) {
  return (
    <Alert className="border-hair border-l-2 border-l-bad bg-muted px-3.5 py-3">
      <CircleAlert className="text-bad" />
      <AlertDescription className="text-[13.5px] leading-[1.6] text-label">{children}</AlertDescription>
    </Alert>
  );
}

function ExplorerLink({ href }: { href: string }) {
  return (
    <TextButton asChild>
      <a href={href} target="_blank" rel="noreferrer">
        View on explorer <ArrowUpRight />
      </a>
    </TextButton>
  );
}

function OnItsWay({ payment }: { payment: Payment | null }) {
  if (payment?.status === "failed") {
    return (
      <Problem>
        This payment couldn&apos;t be sent{payment.failedReason ? `: ${payment.failedReason}` : ""}.
      </Problem>
    );
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
  return (
    <div className="grid justify-items-center gap-3 pt-4 text-center" role="status">
      <span className="grid size-12 place-items-center rounded-full bg-white text-black shadow-[0_0_0_8px_rgba(255,255,255,0.06)]">
        <Check className="size-5" aria-hidden="true" />
      </span>
      <p className="text-2xl font-normal tracking-[-0.02em]">Payment confirmed</p>
      <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
        {order.shop.name} received {amount} USDT on Whitechain.
      </p>
      {order.payment?.explorerUrl && <ExplorerLink href={order.payment.explorerUrl} />}
    </div>
  );
}

function CopyText({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="outline"
      size="xs"
      className="mx-0.5 h-5 border-hair-strong bg-transparent px-2 font-mono text-xs"
      title={text}
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => setCopied(true));
      }}
    >
      {copied ? "copied ✓" : shortAddress(text)}
    </Button>
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
