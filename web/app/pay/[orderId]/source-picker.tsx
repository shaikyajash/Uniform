"use client";

// "Pay with": one row per way to pay, with what it costs and what the wallet holds there. A row
// the wallet can't cover is still shown (the customer may top up), but can't be picked. A split
// row (A8) takes part of the USDC from each chain when no one chain holds enough. A priced token
// (WETH, WBTC) shows an estimate ("≈") at the market price; its quote sets the exact amount.

import { formatTokenAmount } from "@/lib/chain";
import { cn } from "@/lib/utils";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";

/** One chain's part of a split payment, in 6-decimal base units. */
export type SplitPart = { chainId: number; amount: bigint };

export type Source =
  | { kind: "usdt" }
  | { kind: "usdc"; chainId: number }
  | { kind: "split"; parts: SplitPart[] }
  | { kind: "token"; chainId: number; symbol: string };

export type SourceOption = {
  source: Source;
  token: string;
  chain: string;
  /** What this way costs, in `decimals` base units. */
  cost: bigint;
  /** The token's decimals: 6 for USDT and USDC. */
  decimals: number;
  /** The cost is an estimate at the market price (a priced token). */
  approx?: boolean;
  /** What the wallet holds there; `null` while it is being read. */
  balance: bigint | null;
  note?: string;
  /** Why this way can't be used right now, though the wallet may hold enough (no provider can take it). */
  unavailable?: string;
  /** A split row: how much comes from each chain, e.g. "6.00 on Base + 4.03 on Ethereum Sepolia". */
  detail?: string;
};

export const encodeSource = (source: Source) =>
  source.kind === "usdt"
    ? "usdt"
    : source.kind === "usdc"
      ? `usdc:${source.chainId}`
      : source.kind === "token"
        ? `token:${source.chainId}:${source.symbol}`
        : "split";

export function SourcePicker({
  options,
  value,
  onChange,
  disabled,
  feeNote,
}: {
  options: SourceOption[];
  value: Source | null;
  onChange: (source: Source) => void;
  disabled?: boolean;
  /** What USDC prices include: the cheapest provider's fee now, else the highest allowed. */
  feeNote: string;
}) {
  return (
    <div className="grid gap-2">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">PAY WITH</small>
      <RadioGroup
        value={value ? encodeSource(value) : undefined}
        onValueChange={(next) => {
          const option = options.find((candidate) => encodeSource(candidate.source) === next);
          if (option) onChange(option.source);
        }}
        disabled={disabled}
        className="gap-0 overflow-hidden rounded-lg border border-hair"
      >
        {options.map((option) => {
          const id = `source-${encodeSource(option.source)}`;
          const short = option.balance !== null && option.balance < option.cost;
          const blocked = short || option.unavailable !== undefined;
          return (
            <label
              key={id}
              htmlFor={id}
              className={cn(
                "flex cursor-pointer items-center gap-3 border-b border-hair bg-muted/40 px-3.5 py-3 transition-colors last:border-b-0 hover:bg-muted",
                "has-[[data-state=checked]]:bg-white/[0.045]",
                blocked && "cursor-not-allowed opacity-45 hover:bg-muted/40",
              )}
            >
              <RadioGroupItem id={id} value={encodeSource(option.source)} disabled={blocked || disabled} />
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="font-mono text-[13px] text-foreground">{option.token}</span>
                <span className="truncate text-[12.5px] text-muted-foreground">
                  {option.detail ?? `on ${option.chain}`}
                </span>
              </span>
              <span className="grid justify-items-end gap-0.5 text-right">
                <span className="font-mono text-[13px] text-foreground">
                  {option.approx ? "≈" : ""}
                  {formatTokenAmount(option.cost, option.decimals)}
                </span>
                <span className={cn("font-mono text-[10.5px] tracking-[0.04em]", blocked ? "text-warn" : "text-faint")}>
                  {option.unavailable ??
                    (option.balance === null ? "reading balance…" : `balance ${formatTokenAmount(option.balance, option.decimals)}`)}
                </span>
              </span>
            </label>
          );
        })}
      </RadioGroup>
      {options.some((option) => option.note) && (
        <p className="font-mono text-[10px] tracking-[0.06em] text-faint">USDC prices include {feeNote}.</p>
      )}
      {options.some((option) => option.approx) && (
        <p className="font-mono text-[10px] tracking-[0.06em] text-faint">
          ≈ At the market price, fee included. The exact amount is set when you pay.
        </p>
      )}
    </div>
  );
}
