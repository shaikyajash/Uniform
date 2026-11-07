"use client";

// Small pieces the dashboard's tabs share.

import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ConnectedWallet } from "@privy-io/react-auth";
import type { OrderStatus, TypedData } from "@/lib/desk";
import { cn } from "@/lib/utils";

/** A small uppercase label, the dashboard's only heading style below the page title. */
export function Eyebrow({ children, className }: { children: ReactNode; className?: string }) {
  return <p className={cn("font-mono text-[10px] tracking-[0.12em] text-faint uppercase", className)}>{children}</p>;
}

/** A bordered block with a title row. */
export function Panel({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="overflow-hidden rounded-2xl border border-hair bg-gradient-to-b from-panel-2/70 to-panel/70">
      <header className="flex min-h-12 items-center justify-between gap-3 border-b border-hair px-5">
        <Eyebrow className="text-label">{title}</Eyebrow>
        {action}
      </header>
      {children}
    </section>
  );
}

type Tone = "ok" | "warn" | "bad" | "idle" | "live";

const TONE: Record<Tone, string> = {
  ok: "bg-ok",
  warn: "bg-warn",
  bad: "bg-bad",
  idle: "bg-faint",
  live: "bg-white animate-pulse",
};

export function Dot({ tone }: { tone: Tone }) {
  return <span aria-hidden="true" className={cn("inline-block size-1.5 shrink-0 rounded-full", TONE[tone])} />;
}

export const ORDER_STATUS: Record<OrderStatus, { text: string; tone: Tone }> = {
  created: { text: "Waiting", tone: "idle" },
  quoted: { text: "Waiting", tone: "idle" },
  paying: { text: "Paying", tone: "live" },
  source_locked: { text: "Paying", tone: "live" },
  destination_locked: { text: "Paying", tone: "live" },
  paid: { text: "Paid", tone: "ok" },
  partially_refunded: { text: "Part refunded", tone: "warn" },
  refunded: { text: "Refunded", tone: "warn" },
  expired: { text: "Expired", tone: "idle" },
  returned: { text: "Returned", tone: "idle" },
};

/** "12.500000" -> "12.50". */
export function usdt(decimal: string): string {
  const [whole, fraction = ""] = decimal.split(".");
  return `${Number(whole).toLocaleString("en-US")}.${fraction.replace(/0+$/, "").padEnd(2, "0")}`;
}

export function formatTime(unix: number): string {
  const date = new Date(unix * 1000);
  const today = new Date().toDateString() === date.toDateString();
  return today
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { day: "numeric", month: "short" }) +
        " · " +
        date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** Calls `load` now and every `ms` while the page is visible; returns its latest result. */
export function usePolling<T>(load: () => Promise<T>, ms: number, deps: unknown[]): [T | null, () => void] {
  const [value, setValue] = useState<T | null>(null);
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  useEffect(() => {
    loadRef.current = load;
  });
  useEffect(() => {
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = () => {
      if (document.visibilityState === "hidden") {
        timer = setTimeout(run, ms);
        return;
      }
      loadRef.current().then(
        (next) => {
          if (stop) return;
          setValue(next);
          timer = setTimeout(run, ms);
        },
        () => {
          if (!stop) timer = setTimeout(run, ms);
        },
      );
    };
    run();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ms, tick, ...deps]);
  return [value, () => setTick((n) => n + 1)];
}

/** The connected wallet whose address is `address`, if any. */
export function walletFor(wallets: ConnectedWallet[], address: string): ConnectedWallet | undefined {
  return wallets.find((wallet) => wallet.address.toLowerCase() === address.toLowerCase());
}

/** EIP-712 signature from any wallet Privy knows (email wallet or an outside one like MetaMask).
 * Outside wallets only sign typed data for the chain they are on, so it switches first. */
export async function signTypedDataWith(wallet: ConnectedWallet, typedData: TypedData): Promise<string> {
  await wallet.switchChain(Number(typedData.domain.chainId));
  const provider = await wallet.getEthereumProvider();
  return (await provider.request({
    method: "eth_signTypedData_v4",
    params: [wallet.address, JSON.stringify(typedData)],
  })) as string;
}
