"use client";

// Language tabs and copy buttons. The chosen language is shared by every tab group on the page and
// remembered in this browser, so picking Python once shows Python everywhere.

import { useState, useSyncExternalStore } from "react";
import { Check, Copy } from "lucide-react";

export type Variant = { key: string; label: string; code: string; html: string };

const KEY = "uniform.docs.lang";
const listeners = new Set<() => void>();
let memory: string | null = null;

function read(): string | null {
  try {
    return localStorage.getItem(KEY) ?? memory;
  } catch {
    return memory;
  }
}

function choose(key: string) {
  memory = key;
  try {
    localStorage.setItem(KEY, key);
  } catch {
    // Storage blocked: the choice lasts until the page reloads.
  }
  listeners.forEach((listener) => listener());
}

function useChosen(): string | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    read,
    () => null,
  );
}

export function LangTabs({ variants, title }: { variants: Variant[]; title?: string }) {
  const chosen = useChosen();
  // A group without the chosen language (the browser-side samples) shows its first tab.
  const shown = variants.find((variant) => variant.key === chosen) ?? variants[0];
  return (
    <figure className="docs-code my-5 overflow-hidden rounded-md border border-hair bg-[#0b0b0c]">
      <div className="flex items-center justify-between gap-3 border-b border-hair pr-1.5">
        <div role="tablist" aria-label={title ?? "Language"} className="flex min-w-0 overflow-x-auto">
          {variants.map((variant) => (
            <button
              key={variant.key}
              type="button"
              role="tab"
              aria-selected={variant.key === shown.key}
              onClick={() => choose(variant.key)}
              className={`relative shrink-0 px-3.5 py-2.5 font-mono text-[11.5px] transition-colors ${
                variant.key === shown.key
                  ? "text-foreground after:absolute after:inset-x-3.5 after:-bottom-px after:h-px after:bg-foreground"
                  : "text-faint hover:text-label"
              }`}
            >
              {variant.label}
            </button>
          ))}
        </div>
        <CopyCode code={shown.code} />
      </div>
      {title && <p className="border-b border-hair px-4 py-2 font-mono text-[11px] text-faint">{title}</p>}
      <div role="tabpanel" dangerouslySetInnerHTML={{ __html: shown.html }} />
    </figure>
  );
}

export function CopyCode({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label={copied ? "Copied" : "Copy code"}
      onClick={() =>
        void navigator.clipboard.writeText(code).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1400);
        })
      }
      className="grid size-7 shrink-0 place-items-center rounded text-faint transition-colors hover:bg-white/[0.06] hover:text-foreground"
    >
      {copied ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
    </button>
  );
}
