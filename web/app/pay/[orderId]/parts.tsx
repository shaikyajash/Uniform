"use client";

// The pay page's building blocks, styled like the home page: the card, its motion, buttons,
// notices and small text. The flow itself is in pay-card.tsx.

import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowUpRight, Check, CircleAlert, Copy, TriangleAlert } from "lucide-react";
import { shortAddress } from "@/lib/chain";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

/* ---------- Motion, as on Garden: the card glides to its new height and its contents crossfade ---------- */

const GLIDE = { duration: 0.35, ease: [0.22, 1, 0.36, 1] } as const;

/** Animates its height to whatever its content measures, instead of snapping. */
export function AutoHeight({ children }: { children: ReactNode }) {
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
export function Swap({ id, children }: { id: string; children: ReactNode }) {
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
export function Reveal({ show, children }: { show: boolean; children: ReactNode }) {
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

export function SkeletonBody() {
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
export function PayShell({ children, busy = false }: { children: ReactNode; busy?: boolean }) {
  return (
    <MotionConfig reducedMotion="user">
      <Card
        aria-busy={busy}
        className="w-full max-w-[420px] gap-0 rounded-2xl bg-gradient-to-b from-panel-2 to-panel py-6 ring-hair-strong shadow-[inset_0_1px_0_rgba(255,255,255,0.12),0_40px_80px_-40px_#000] [--card-spacing:--spacing(6)] embedded:max-w-none embedded:rounded-none embedded:bg-transparent embedded:bg-none embedded:py-4 embedded:shadow-none embedded:ring-0 embedded:[--card-spacing:--spacing(4)]"
      >
        <AutoHeight>{children}</AutoHeight>
      </Card>
    </MotionConfig>
  );
}

/** The home page's white pill, with its uppercase system-font text. */
export function PrimaryButton(props: React.ComponentProps<typeof Button>) {
  return (
    <Button
      className="h-11 w-full rounded-full border-white/30 font-system text-sm font-medium uppercase hover:bg-white/90 disabled:opacity-35 embedded:h-8 embedded:text-[11.5px] embedded:tracking-[0.03em]"
      {...props}
    />
  );
}

/** Small mono uppercase link, like the panel bars on the home page. */
export function TextButton(props: React.ComponentProps<typeof Button>) {
  return (
    <Button
      variant="link"
      className="h-auto p-0 font-mono text-[10.5px] tracking-[0.08em] text-label uppercase underline decoration-faint underline-offset-[3px] hover:decoration-foreground"
      {...props}
    />
  );
}

/** A label over a value, like the stats row under the home page's cross-chain example. */
export function Stat({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid gap-1">
      <small className="font-mono text-[9px] tracking-[0.1em] text-faint">{label}</small>
      <b className="font-mono text-[15px] font-normal text-[#d9d9de]">{children}</b>
    </div>
  );
}

export function FinePrint({ children }: { children: ReactNode }) {
  return (
    <p className="text-center font-mono text-[10.5px] tracking-[0.08em] text-faint uppercase embedded:text-[9px]">{children}</p>
  );
}

/** Something the customer can fix (not enough USDT, the time ran out). */
export function Notice({ children }: { children: ReactNode }) {
  return (
    <Alert className="border-hair border-l-2 border-l-warn bg-muted px-3.5 py-3">
      <TriangleAlert className="text-warn" />
      <AlertDescription className="text-[13.5px] leading-[1.6] text-label">{children}</AlertDescription>
    </Alert>
  );
}

/** Something went wrong. */
export function Problem({ children }: { children: ReactNode }) {
  return (
    <Alert className="border-hair border-l-2 border-l-bad bg-muted px-3.5 py-3">
      <CircleAlert className="text-bad" />
      <AlertDescription className="text-[13.5px] leading-[1.6] text-label">{children}</AlertDescription>
    </Alert>
  );
}

/** `label`: which chain, when there are several links (a split's locks). */
export function ExplorerLink({ href, label }: { href: string; label?: string }) {
  return (
    <TextButton asChild>
      <a href={href} target="_blank" rel="noreferrer">
        {label ? `${label} explorer` : "View on explorer"} <ArrowUpRight />
      </a>
    </TextButton>
  );
}

/**
 * Copies text. The Clipboard API is missing on some pages (plain http, some embedded frames) and
 * can refuse a write, so fall back to selecting a hidden field and running the copy command.
 */
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to the older method below.
  }
  const field = document.createElement("textarea");
  field.value = text;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.appendChild(field);
  field.select();
  try {
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    field.remove();
  }
}

export function CopyText({ text }: { text: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  return (
    <Button
      variant="outline"
      size="xs"
      className="mx-0.5 h-5 border-hair-strong bg-transparent px-2 font-mono text-xs"
      title={text}
      onClick={async () => {
        const ok = await copyText(text);
        setState(ok ? "copied" : "failed");
        setTimeout(() => setState("idle"), 2000);
      }}
    >
      {state === "copied" ? "copied ✓" : state === "failed" ? "copy failed, select it" : shortAddress(text)}
    </Button>
  );
}

/** A small copy icon to sit beside a value shown in full elsewhere (the wallet address). */
export function CopyIcon({ text, label }: { text: string; label: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const title = state === "copied" ? "Copied" : state === "failed" ? "Couldn't copy, select it instead" : label;
  return (
    <button
      type="button"
      aria-label={title}
      title={title}
      className="inline-grid size-6 place-items-center rounded-md text-faint transition-colors hover:bg-white/8 hover:text-foreground focus-visible:outline focus-visible:outline-1 focus-visible:outline-white/50"
      onClick={async () => {
        const ok = await copyText(text);
        setState(ok ? "copied" : "failed");
        setTimeout(() => setState("idle"), 2000);
      }}
    >
      {state === "copied" ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
    </button>
  );
}

/** Unix seconds, ticking once a second. */
export function useNow(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

export function formatCountdown(seconds: number): string {
  const safe = Math.max(0, seconds);
  return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, "0")}`;
}
