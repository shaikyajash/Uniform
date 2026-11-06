"use client";

// The liquidity provider's page: pick a wallet (no signing in: the address is the account), then
// see what Uniform's pool may take from it, what it paid out, and add or withdraw liquidity.

import {
  useConnectWallet,
  usePrivy,
  useWallets,
  type ConnectedWallet,
} from "@privy-io/react-auth";
import { useCallback, useEffect, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  Loader2,
  Wallet,
} from "lucide-react";
import Link from "next/link";
import { toast } from "sonner";
import { formatUnits, parseUnits, type Address } from "viem";
import { Eyebrow, Panel } from "@/app/dashboard/bits";
import { CopyIcon } from "@/app/pay/[orderId]/parts";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  explorerTx,
  formatUsdt,
  providerFeeBps,
  providerFeeRange,
  shopChain,
  shortAddress,
} from "@/lib/chain";
import {
  MIN_GAS_WBT,
  WBT_FAUCET,
  addLiquidity,
  estimatedFees,
  feeAllowed,
  payableNow,
  readPosition,
  startProviding,
  withdraw,
  type OnStep,
  type Position,
  type Shipped,
} from "@/lib/earn";

/** "0.1% to 1%": the fees a provider may ask. */
const FEE_RANGE = `${providerFeeRange.min / 100}% to ${providerFeeRange.max / 100}%`;
const PILL =
  "h-10 rounded-full font-mono text-[11px] tracking-[0.08em] uppercase";

export function Earn() {
  const { wallets } = useWallets();
  const [chosen, setChosen] = useState<string | null>(null);
  const wallet = wallets.find((w) => w.address === chosen) ?? null;
  return wallet ? (
    <Provider
      key={wallet.address}
      wallet={wallet}
      onSwitch={() => setChosen(null)}
    />
  ) : (
    <Connect wallets={wallets} onPick={setChosen} />
  );
}

function Connect({
  wallets,
  onPick,
}: {
  wallets: ConnectedWallet[];
  onPick: (address: string) => void;
}) {
  const { ready, login, authenticated } = usePrivy();
  const { connectWallet } = useConnectWallet();
  return (
    <div className="mx-auto grid w-full max-w-[440px] gap-8">
      <div className="grid gap-3">
        <Back href="/">Back</Back>
        <Eyebrow>Earn</Eyebrow>
        <h1 className="font-display text-[40px] leading-[1.05] tracking-[-0.03em]">
          Lend USDT to
          <br />
          <span className="text-muted-foreground">merchant payouts</span>
        </h1>
        <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
          Your USDT stays in your wallet. Uniform takes it only to pay a
          merchant for a customer payment that is already locked, and you earn
          the fee you set ({FEE_RANGE}) each time. Each payment goes to the
          lowest fee that can cover it.
        </p>
      </div>

      <div className="overflow-hidden rounded-2xl border border-hair bg-panel">
        {wallets.length > 0 && (
          <ul className="divide-y divide-hair">
            {wallets.map((w) => (
              <li key={w.address}>
                <button
                  type="button"
                  onClick={() => onPick(w.address)}
                  className="group flex w-full items-center gap-3 px-5 py-4 text-left transition-colors hover:bg-white/[0.03]"
                >
                  <span className="grid size-8 place-items-center rounded-full border border-hair-strong text-muted-foreground">
                    <Wallet className="size-3.5" />
                  </span>
                  <span className="grid flex-1 gap-0.5">
                    <span className="font-mono text-[13.5px] text-foreground">
                      {shortAddress(w.address)}
                    </span>
                    <span className="text-[12px] text-faint">
                      {w.walletClientType === "privy"
                        ? "Email wallet"
                        : w.walletClientType}
                    </span>
                  </span>
                  <span className="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] text-label uppercase opacity-70 transition-opacity group-hover:opacity-100">
                    Open <ArrowRight className="size-3.5" />
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
        <div
          className={
            wallets.length > 0
              ? "flex gap-2 border-t border-hair p-3"
              : "grid gap-2 p-3"
          }
        >
          <Button
            className={`${PILL} flex-1 bg-white text-black hover:bg-white/90`}
            disabled={!ready}
            onClick={() => connectWallet()}
          >
            {wallets.length > 0 ? "Another wallet" : "Connect wallet"}
          </Button>
          {!authenticated && (
            <Button
              variant="outline"
              className={`${PILL} flex-1 border-hair-strong bg-transparent`}
              disabled={!ready}
              onClick={() => login()}
            >
              Email wallet
            </Button>
          )}
        </div>
      </div>
      <p className="text-[12.5px] leading-[1.6] text-faint">
        Nothing to sign to look. Your wallet asks only when you add or withdraw.
      </p>
    </div>
  );
}

type Load = { position: Position | null; problem: string | null };

function Provider({
  wallet,
  onSwitch,
}: {
  wallet: ConnectedWallet;
  onSwitch: () => void;
}) {
  const address = wallet.address as Address;
  const [load, setLoad] = useState<Load>({ position: null, problem: null });
  const [tick, setTick] = useState(0);
  const refresh = useCallback(() => setTick((n) => n + 1), []);

  useEffect(() => {
    let stop = false;
    const run = () =>
      readPosition(address).then(
        (position) => !stop && setLoad({ position, problem: null }),
        (error: unknown) =>
          !stop && setLoad((old) => ({ ...old, problem: plain(error) })),
      );
    void run();
    const timer = window.setInterval(
      () => document.visibilityState === "visible" && void run(),
      20_000,
    );
    return () => {
      stop = true;
      window.clearInterval(timer);
    };
  }, [address, tick]);

  const { position, problem } = load;
  const live =
    position?.strategies.filter((s) => !s.docked && s.forUniform) ?? [];
  const closed = position?.strategies.filter((s) => s.docked).length ?? 0;
  // Strategies for another filler (old tests, or other apps) that still hold a limit.
  const foreign =
    position?.strategies.filter(
      (s) => !s.docked && !s.forUniform && s.balance > BigInt(0),
    ).length ?? 0;
  const shipped = live.reduce((sum, s) => sum + s.balance, BigInt(0));

  return (
    <div className="mx-auto grid w-full max-w-[1080px] gap-8">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div className="grid gap-2">
          <Back onClick={onSwitch}>Wallets</Back>
          <Eyebrow>Liquidity provider</Eyebrow>
          <div className="flex items-center gap-2">
            <h1
              className="font-mono text-[28px] leading-none tracking-[-0.02em] sm:text-[34px]"
              title={address}
            >
              {shortAddress(address)}
            </h1>
            <CopyIcon text={address} label="Copy wallet address" />
          </div>
          <p className="text-[13px] text-muted-foreground">
            USDT on Whitechain · your fee on every payout
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="font-mono text-[10.5px] tracking-[0.08em] text-muted-foreground uppercase hover:text-foreground"
          onClick={onSwitch}
        >
          Switch wallet
        </Button>
      </header>

      {problem && (
        <p
          role="alert"
          className="border-l-2 border-bad pl-3 text-[13.5px] leading-[1.6] text-label"
        >
          Couldn&apos;t read your position: {problem}
        </p>
      )}

      <dl className="grid grid-cols-2 overflow-hidden rounded-2xl border border-hair lg:grid-cols-4">
        <Stat
          label="Ready to pay out"
          value={position ? payableNow(position) : null}
          hint="What the pool can take right now"
        />
        <Stat
          label="Lent to Uniform"
          value={position ? shipped : null}
          hint="The limit you set"
        />
        <Stat
          label="In your wallet"
          value={position?.walletUsdt ?? null}
          hint="USDT on Whitechain"
        />
        <Stat
          label="Fees earned"
          value={position ? estimatedFees(position) : null}
          hint="Your fee on what you paid out"
        />
      </dl>

      {position && <Warnings position={position} shipped={shipped} />}

      <Panel title="Your liquidity">
        {!position ? (
          <Loading />
        ) : live.length === 0 ? (
          <Start wallet={wallet} position={position} onDone={refresh} />
        ) : (
          <ul className="divide-y divide-hair">
            {live.map((s) => (
              <StrategyRow
                key={s.hash}
                wallet={wallet}
                position={position}
                shipped={s}
                onDone={refresh}
              />
            ))}
          </ul>
        )}
        {(closed > 0 || foreign > 0) && (
          <p className="border-t border-hair px-5 py-3 text-[12.5px] text-faint">
            {closed > 0 &&
              `${closed} withdrawn ${closed === 1 ? "strategy" : "strategies"}.`}
            {closed > 0 && foreign > 0 && " "}
            {foreign > 0 &&
              `${foreign} live ${foreign === 1 ? "strategy isn't" : "strategies aren't"} set up for Uniform's pool.`}
          </p>
        )}
      </Panel>

      <Panel title="Payouts from your liquidity">
        {!position ? (
          <Loading />
        ) : position.fills.length === 0 ? (
          <p className="px-5 py-8 text-[14px] leading-[1.7] text-muted-foreground">
            No payouts yet. When a customer pays from Base or Ethereum, Uniform
            pays the merchant from your USDT and you earn your fee, when yours
            is the lowest that can cover the payment.
          </p>
        ) : (
          <ul className="divide-y divide-hair">
            {position.fills.map((fill) => (
              <li
                key={fill.lockId}
                className="flex items-center justify-between gap-4 px-5 py-3.5"
              >
                <span className="grid min-w-0 gap-0.5">
                  <span className="text-[14px]">
                    {formatUsdt(fill.amount)} USDT
                  </span>
                  <span className="truncate font-mono text-[11.5px] text-faint">
                    Order {shortAddress(fill.orderId)}
                  </span>
                </span>
                <a
                  href={explorerTx(shopChain.id, fill.tx)}
                  target="_blank"
                  rel="noreferrer"
                  className="flex shrink-0 items-center gap-1 font-mono text-[10.5px] tracking-[0.08em] text-label uppercase hover:text-foreground"
                >
                  Transaction <ArrowUpRight className="size-3" />
                </a>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function Stat({
  label,
  value,
  hint,
}: {
  label: string;
  value: bigint | null;
  hint: string;
}) {
  return (
    <div className="grid gap-2 border-hair p-5 [&:not(:nth-child(2n))]:border-r lg:[&:not(:last-child)]:border-r [&:nth-child(-n+2)]:border-b lg:[&:nth-child(-n+2)]:border-b-0">
      <dt>
        <Eyebrow>{label}</Eyebrow>
      </dt>
      <dd className="grid gap-1">
        <span className="text-[24px] leading-none tracking-[-0.02em]">
          {value === null ? (
            <span className="text-faint">—</span>
          ) : (
            formatUsdt(value)
          )}
          <span className="ml-1.5 font-mono text-[11px] text-faint">USDT</span>
        </span>
        <span className="text-[12px] text-faint">{hint}</span>
      </dd>
    </div>
  );
}

/** Why the pool may be able to take less than the limit. */
function Warnings({
  position,
  shipped,
}: {
  position: Position;
  shipped: bigint;
}) {
  if (shipped === BigInt(0)) return null;
  const notes: string[] = [];
  if (position.walletUsdt < shipped)
    notes.push(
      "Your wallet holds less USDT than you lent, so the pool can only pay out what's there.",
    );
  if (position.allowance < shipped)
    notes.push(
      "Your wallet lets Aqua move less than you lent. Adding liquidity approves the rest.",
    );
  if (notes.length === 0) return null;
  return (
    <div className="grid gap-1 border-l-2 border-warn pl-3 text-[13.5px] leading-[1.6] text-label">
      {notes.map((note) => (
        <p key={note}>{note}</p>
      ))}
    </div>
  );
}

function Loading() {
  return (
    <p className="flex items-center gap-2 px-5 py-8 text-[14px] text-muted-foreground">
      <Loader2 className="size-4 animate-spin" /> Reading Whitechain…
    </p>
  );
}

/** No live strategy yet: lend an amount, which ships a new strategy to Uniform's pool. */
function Start({
  wallet,
  position,
  onDone,
}: {
  wallet: ConnectedWallet;
  position: Position;
  onDone: () => void;
}) {
  const [feeText, setFeeText] = useState(String(providerFeeBps / 100));
  const feeBps = Math.round(Number(feeText) * 100);
  const feeOk = feeText.trim() !== "" && feeAllowed(feeBps);
  return (
    <div className="grid gap-4 px-5 py-6">
      <p className="max-w-[560px] text-[14px] leading-[1.7] text-muted-foreground">
        Choose how much USDT Uniform may use and the fee you ask. It stays in
        your wallet; you can withdraw the permission at any time. Each payment
        goes to the lowest fee that can cover it, so a lower fee wins more
        payments.
      </p>
      <div className="grid max-w-[420px] gap-2">
        <div className="relative">
          <Input
            inputMode="decimal"
            value={feeText}
            onChange={(event) => setFeeText(event.target.value)}
            aria-label="Your fee in percent"
            aria-invalid={!feeOk}
            className="h-10 rounded-full border-hair-strong pr-24 pl-4 font-mono aria-invalid:border-bad/60"
          />
          <span className="pointer-events-none absolute top-1/2 right-4 -translate-y-1/2 font-mono text-[11px] text-faint">
            % FEE
          </span>
        </div>
        <p
          className={`pl-4 text-[12.5px] ${feeOk ? "text-faint" : "text-bad"}`}
          aria-live="polite"
        >
          {feeOk
            ? `You earn ${feeBps / 100}% of each payout · allowed ${FEE_RANGE}`
            : `Set a fee from ${FEE_RANGE}`}
        </p>
      </div>
      <AmountForm
        action="Lend"
        busyText="Lending…"
        max={position.walletUsdt}
        gas={position.walletGas}
        disabled={!feeOk}
        run={(amount, onStep) =>
          startProviding(wallet, position, amount, feeBps, onStep)
        }
        done={(amount) => `Lent ${formatUsdt(amount)} USDT to Uniform`}
        onDone={onDone}
      />
    </div>
  );
}

function StrategyRow({
  wallet,
  position,
  shipped,
  onDone,
}: {
  wallet: ConnectedWallet;
  position: Position;
  shipped: Shipped;
  onDone: () => void;
}) {
  const [closing, setClosing] = useState(false);
  return (
    <li className="grid gap-4 px-5 py-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-1">
          <span className="text-[20px] leading-none tracking-[-0.02em]">
            {formatUsdt(shipped.balance)}{" "}
            <span className="font-mono text-[11px] text-faint">USDT</span>
          </span>
          <span className="text-[12.5px] text-faint">
            Up to {formatUsdt(shipped.strategy.maxPerOrder)} per order ·{" "}
            {shipped.strategy.feeBps / 100}% fee ·{" "}
            <a
              href={explorerTx(shopChain.id, shipped.shippedTx)}
              target="_blank"
              rel="noreferrer"
              className="font-mono hover:text-foreground"
            >
              {shortAddress(shipped.hash)}
            </a>
          </span>
        </div>
        <Button
          variant="outline"
          className={`${PILL} h-9 border-hair-strong bg-transparent`}
          disabled={closing}
          onClick={() => {
            setClosing(true);
            withdraw(wallet, position, shipped).then(
              () => {
                toast.success(
                  "Withdrawn. Uniform can't use this USDT anymore.",
                );
                onDone();
              },
              (error: unknown) => {
                toast.error(plain(error));
                setClosing(false);
              },
            );
          }}
        >
          {closing ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            "Withdraw all"
          )}
        </Button>
      </div>
      <AmountForm
        action="Add"
        busyText="Adding…"
        max={unlent(position)}
        gas={position.walletGas}
        run={(amount, onStep) =>
          addLiquidity(wallet, position, shipped, amount, onStep)
        }
        done={(amount) => `Added ${formatUsdt(amount)} USDT`}
        onDone={onDone}
      />
    </li>
  );
}

function AmountForm({
  action,
  busyText,
  max,
  gas,
  disabled = false,
  run,
  done,
  onDone,
}: {
  action: string;
  busyText: string;
  /** The most that can be lent: lending more than the wallet holds could never be paid out. */
  max: bigint;
  /** WBT in the wallet: without it the wallet can't send the transaction. */
  gas: bigint;
  /** Another field on the page is not valid yet. */
  disabled?: boolean;
  /** Sends the transactions, reporting each step as its wallet window opens. */
  run: (amount: bigint, onStep: OnStep) => Promise<unknown>;
  done: (amount: bigint) => string;
  onDone: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<string | null>(null);
  const amount = toUnits(text);
  const problem = amountProblem(text, amount, max);
  const usable = !disabled && amount !== null && problem === null && gas >= MIN_GAS_WBT;
  if (max < MIN_LEND) {
    return (
      <p className="text-[13.5px] leading-[1.6] text-muted-foreground">
        {max <= BigInt(0)
          ? "All the USDT in this wallet is already lent. Add USDT to the wallet on Whitechain to lend more."
          : `You can lend from ${formatUsdt(MIN_LEND)} USDT; this wallet has ${formatUsdt(max)} USDT free. Add USDT on Whitechain to lend.`}
      </p>
    );
  }
  if (gas < MIN_GAS_WBT) {
    return (
      <p className="text-[13.5px] leading-[1.6] text-muted-foreground">
        This wallet needs a little WBT to pay gas on Whitechain (
        {formatUnits(gas, 18).slice(0, 8)} WBT now). Get 0.5 WBT free at{" "}
        <a
          href={WBT_FAUCET}
          target="_blank"
          rel="noreferrer"
          className="text-foreground underline decoration-hair-strong underline-offset-4 hover:decoration-foreground"
        >
          faucet.testnet.whitechain.io
        </a>
        , then reload.
      </p>
    );
  }

  return (
    <div className="grid max-w-[420px] gap-2">
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (!usable || !amount) return;
          setBusy(true);
          run(amount, setStep).then(
            () => {
              toast.success(done(amount));
              setText("");
              setBusy(false);
              setStep(null);
              onDone();
            },
            (error: unknown) => {
              toast.error(plain(error));
              setBusy(false);
              setStep(null);
            },
          );
        }}
      >
        <div className="relative flex-1">
          <Input
            inputMode="decimal"
            placeholder="0.00"
            value={text}
            onChange={(event) => setText(event.target.value)}
            disabled={busy}
            aria-label="Amount in USDT"
            aria-invalid={problem !== null}
            className="h-10 rounded-full border-hair-strong pr-24 pl-4 font-mono aria-invalid:border-bad/60"
          />
          <span className="pointer-events-none absolute top-1/2 right-14 -translate-y-1/2 font-mono text-[11px] text-faint">
            USDT
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={() => setText(formatUnits(max, 6))}
            className="absolute top-1/2 right-2 -translate-y-1/2 rounded-full px-2.5 py-1 font-mono text-[10.5px] tracking-[0.06em] text-label uppercase hover:bg-white/[0.06] hover:text-foreground"
          >
            Max
          </button>
        </div>
        <Button
          type="submit"
          className={`${PILL} bg-white px-5 text-black hover:bg-white/90`}
          disabled={busy || !usable}
        >
          {busy ? (
            <>
              <Loader2 className="size-3.5 animate-spin" /> {busyText}
            </>
          ) : (
            action
          )}
        </Button>
      </form>
      <p
        className={`pl-4 text-[12.5px] ${problem ? "text-bad" : "text-faint"}`}
        aria-live="polite"
      >
        {busy && step
          ? `${step}. Confirm in the wallet window.`
          : (problem ??
            `From ${formatUsdt(MIN_LEND)} up to ${formatUsdt(max)} USDT`)}
      </p>
    </div>
  );
}

/** The least a lend or top-up may be: less barely covers one order. Our choice, not a contract rule. */
const MIN_LEND = BigInt(1_000_000);

/** USDT in the wallet not already lent to Uniform: what more could be backed. */
function unlent(position: Position): bigint {
  const lent = position.strategies
    .filter((s) => !s.docked && s.forUniform)
    .reduce((sum, s) => sum + s.balance, BigInt(0));
  const free = position.walletUsdt - lent;
  return free > BigInt(0) ? free : BigInt(0);
}

/** Why an amount can't be lent, in plain words, or null when it can (or nothing is typed yet). */
function amountProblem(
  text: string,
  amount: bigint | null,
  max: bigint,
): string | null {
  if (text.trim() === "") return null;
  if (amount === null) return "Enter an amount like 5 or 2.5.";
  if (amount < MIN_LEND) return `At least ${formatUsdt(MIN_LEND)} USDT.`;
  if (amount > max) return `You have ${formatUsdt(max)} USDT free to lend.`;
  return null;
}

/** "12.5" -> 12500000 base units; null unless it is a positive amount with at most 6 decimals. */
function toUnits(text: string): bigint | null {
  if (!/^\d+(\.\d{0,6})?$/.test(text.trim())) return null;
  const units = parseUnits(text.trim(), 6);
  return units > BigInt(0) ? units : null;
}

/** A wallet or chain error, in plain words. */
function plain(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (/reject|denied|cancel/i.test(message))
    return "You cancelled in your wallet.";
  if (/insufficient funds|gas required exceeds/i.test(message))
    return "Your wallet needs a little WBT on Whitechain to pay gas.";
  return message.split("\n")[0].slice(0, 160);
}

/** A small "← Back" above a page's title: a link, or a button for a step within the page. */
function Back({
  href,
  onClick,
  children,
}: {
  href?: string;
  onClick?: () => void;
  children: string;
}) {
  const className =
    "mb-2 inline-flex w-fit items-center gap-1.5 font-mono text-[10.5px] tracking-[0.08em] text-muted-foreground uppercase transition-colors hover:text-foreground";
  const content = (
    <>
      <ArrowLeft className="size-3.5" aria-hidden="true" /> {children}
    </>
  );
  return href ? (
    <Link href={href} className={className}>
      {content}
    </Link>
  ) : (
    <button type="button" onClick={onClick} className={className}>
      {content}
    </button>
  );
}
