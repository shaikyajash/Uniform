"use client";

// A cross-chain payment's three steps, as the customer lives through them. Each step shows done,
// working or waiting, and a transaction link once there is one.

import { formatUnits } from "viem";
import { Check, Loader2 } from "lucide-react";
import { chainName, explorerTx, trimAmount } from "@/lib/chain";
import { currentLegs, type Leg, type Order } from "@/lib/desk";
import { cn } from "@/lib/utils";
import { ExplorerLink } from "./parts";

type State = "done" | "working" | "waiting";

const RANK: Record<string, number> = { paying: 0, source_locked: 1, destination_locked: 2, paid: 3 };

/** "6.00 USDC on Base" for one leg; "6.00 on Base and 4.03 USDC on Ethereum Sepolia" for a split. */
export function describeLegs(legs: Leg[]): string {
  const parts = legs.map((leg) => `${trimAmount(formatUnits(BigInt(leg.amount), 6))} USDC on ${chainName(leg.chainId)}`);
  if (parts.length === 1) return parts[0];
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

export function Progress({ order, amount }: { order: Order; amount: string }) {
  const legs = currentLegs(order);
  const leg = legs[0];
  const split = legs.length > 1;
  const rank = RANK[order.status] ?? 0;
  const lockTx = leg?.lockTx ?? order.payment?.txHash ?? null;
  const chain = leg ? chainName(leg.chainId) : "";
  const lockedCount = legs.filter((each) => each.status !== "pending").length;
  const claim = order.claim;

  const steps: {
    title: string;
    detail: string;
    state: State;
    link?: string | null;
    links?: { label: string; href: string }[];
  }[] = [
    {
      title: legs.length > 0 ? `Lock ${describeLegs(legs)}` : "Lock your USDC",
      detail:
        rank > 0
          ? split
            ? "Both held for the provider. They come back to you if the shop isn't paid."
            : "Held for the provider. It comes back to you if the shop isn't paid."
          : split
            ? `${lockedCount} of ${legs.length} locked. The provider waits for all of them.`
            : order.payment?.status === "sent"
              ? `Waiting for ${chain} to confirm…`
              : "Sending your signed note…",
      state: rank > 0 ? "done" : "working",
      link: !split && lockTx && leg ? explorerTx(leg.chainId, lockTx) : null,
      links: split
        ? legs.filter((each) => each.lockTx).map((each) => ({ label: chainName(each.chainId), href: explorerTx(each.chainId, each.lockTx as string) }))
        : undefined,
    },
    {
      title: `Provider locks ${amount} USDT for ${order.shop.name}`,
      detail: rank > 1 ? "Locked on Whitechain, waiting for your key." : "Usually under a minute.",
      state: rank > 1 ? "done" : rank === 1 ? "working" : "waiting",
    },
    {
      title: "Pay the shop",
      detail:
        rank > 2
          ? "Paid."
          : claim?.status === "sent" || claim?.status === "sending"
            ? "Releasing the USDT to the shop…"
            : rank === 2
              ? "Checking the shop's lock, then releasing it…"
              : "Your key releases the shop's USDT.",
      state: rank > 2 ? "done" : rank === 2 ? "working" : "waiting",
      link: claim?.explorerUrl,
    },
  ];

  return (
    <ol className="grid gap-0 rounded-lg border border-hair bg-muted/40 px-3.5 py-1" aria-live="polite">
      {steps.map((step, index) => (
        <li key={step.title} className="relative flex gap-3 py-3">
          {index < steps.length - 1 && (
            <span
              aria-hidden="true"
              className={cn(
                "absolute top-[34px] bottom-[-10px] left-[9px] w-px",
                step.state === "done" ? "bg-white/40" : "bg-hair",
              )}
            />
          )}
          <Marker state={step.state} />
          <div className="grid min-w-0 flex-1 gap-1">
            <p className={cn("text-[14px] leading-snug", step.state === "waiting" ? "text-faint" : "text-foreground")}>
              {step.title}
            </p>
            <p className="text-[12.5px] leading-[1.55] text-muted-foreground">{step.detail}</p>
            {step.link && (
              <div>
                <ExplorerLink href={step.link} />
              </div>
            )}
            {step.links && step.links.length > 0 && (
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {step.links.map((link) => (
                  <ExplorerLink key={link.href} href={link.href} label={link.label} />
                ))}
              </div>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}

function Marker({ state }: { state: State }) {
  if (state === "done") {
    return (
      <span className="mt-0.5 grid size-[19px] shrink-0 place-items-center rounded-full bg-white text-black">
        <Check className="size-3" strokeWidth={3} aria-hidden="true" />
      </span>
    );
  }
  if (state === "working") {
    return (
      <span className="mt-0.5 grid size-[19px] shrink-0 place-items-center rounded-full border border-white/50">
        <Loader2 className="size-3 animate-spin text-foreground" aria-hidden="true" />
      </span>
    );
  }
  return <span className="mt-0.5 size-[19px] shrink-0 rounded-full border border-hair-strong" aria-hidden="true" />;
}
