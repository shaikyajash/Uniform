"use client";

// "Pay with": one row per way to pay, with what it costs and what the wallet holds there. A row
// the wallet can't cover is still shown (the customer may top up), but can't be picked.

import { formatUnits } from "viem";
import { trimAmount } from "@/lib/chain";
import { cn } from "@/lib/utils";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";

export type Source = { kind: "usdt" } | { kind: "usdc"; chainId: number };

export type SourceOption = {
  source: Source;
  token: "USDT" | "USDC";
  chain: string;
  /** What this way costs, in 6-decimal base units. */
  cost: bigint;
  /** What the wallet holds there; `null` while it is being read. */
  balance: bigint | null;
  note?: string;
};

const encode = (source: Source) => (source.kind === "usdt" ? "usdt" : `usdc:${source.chainId}`);
const decode = (value: string): Source =>
  value === "usdt" ? { kind: "usdt" } : { kind: "usdc", chainId: Number(value.slice(5)) };
const show = (units: bigint) => trimAmount(formatUnits(units, 6));

export function SourcePicker({
  options,
  value,
  onChange,
  disabled,
}: {
  options: SourceOption[];
  value: Source | null;
  onChange: (source: Source) => void;
  disabled?: boolean;
}) {
  return (
    <div className="grid gap-2">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">PAY WITH</small>
      <RadioGroup
        value={value ? encode(value) : undefined}
        onValueChange={(next) => onChange(decode(next))}
        disabled={disabled}
        className="gap-0 overflow-hidden rounded-lg border border-hair"
      >
        {options.map((option) => {
          const id = `source-${encode(option.source)}`;
          const short = option.balance !== null && option.balance < option.cost;
          return (
            <label
              key={id}
              htmlFor={id}
              className={cn(
                "flex cursor-pointer items-center gap-3 border-b border-hair bg-muted/40 px-3.5 py-3 transition-colors last:border-b-0 hover:bg-muted",
                "has-[[data-state=checked]]:bg-white/[0.045]",
                short && "cursor-not-allowed opacity-45 hover:bg-muted/40",
              )}
            >
              <RadioGroupItem id={id} value={encode(option.source)} disabled={short || disabled} />
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="font-mono text-[13px] text-foreground">{option.token}</span>
                <span className="truncate text-[12.5px] text-muted-foreground">on {option.chain}</span>
              </span>
              <span className="grid justify-items-end gap-0.5 text-right">
                <span className="font-mono text-[13px] text-foreground">{show(option.cost)}</span>
                <span className={cn("font-mono text-[10.5px] tracking-[0.04em]", short ? "text-warn" : "text-faint")}>
                  {option.balance === null ? "reading balance…" : `balance ${show(option.balance)}`}
                </span>
              </span>
            </label>
          );
        })}
      </RadioGroup>
      {options.some((option) => option.note) && (
        <p className="font-mono text-[10px] tracking-[0.06em] text-faint">USDC prices include the provider&apos;s 0.3% fee.</p>
      )}
    </div>
  );
}
