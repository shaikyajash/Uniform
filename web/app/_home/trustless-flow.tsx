"use client";

// "How we are trustless": the steps of one cross-chain payment, in the order they happen, on the
// left; the contracts and the wires the money moves along, on the right. The steps play one by
// one; hovering (or focusing) a step holds it, and leaving carries on from there.

import { useEffect, useRef, useState, type CSSProperties } from "react";

import styles from "./home.module.css";

type NodeKey = "customer" | "source" | "provider" | "destination" | "merchant" | "aqua";
type WireKey = "customer-source" | "source-provider" | "provider-destination" | "aqua-destination" | "destination-merchant";

/** Where each node sits, in % of the diagram. */
/** `wide`: the logo is a wordmark, so it keeps its own shape instead of a square. */
const NODES: Record<NodeKey, { x: number; y: number; label: string; logo: string; wide?: boolean }> = {
  customer: { x: 8, y: 50, label: "Customer", logo: "/logos/usdc.svg" },
  source: { x: 34, y: 50, label: "Source lock", logo: "/logos/base.png" },
  provider: { x: 50, y: 14, label: "Provider", logo: "/logos/uniform-mark.png" },
  destination: { x: 66, y: 50, label: "Destination lock", logo: "/logos/whitechain-icon.png" },
  merchant: { x: 92, y: 50, label: "Merchant", logo: "/logos/usdt.svg" },
  aqua: { x: 66, y: 86, label: "1inch Aqua", logo: "/logos/1inch.svg", wide: true },
};

/** Each wire runs from its first node to its second; `bend` is the curve's corner, in %. */
const WIRES: Record<WireKey, { from: NodeKey; to: NodeKey; bend?: { x: number; y: number } }> = {
  "customer-source": { from: "customer", to: "source" },
  "source-provider": { from: "source", to: "provider", bend: { x: 34, y: 14 } },
  "provider-destination": { from: "provider", to: "destination", bend: { x: 66, y: 14 } },
  "aqua-destination": { from: "aqua", to: "destination" },
  "destination-merchant": { from: "destination", to: "merchant" },
};

type Step = {
  title: string;
  /** What happens, in a sentence or two; shown under the active step. */
  says: string;
  nodes: NodeKey[];
  /** The wires money moves along; `back` runs the light from the wire's end to its start. */
  flows: { wire: WireKey; back?: boolean }[];
  /** Not part of the happy path: shown after a divider. */
  fallback?: boolean;
};

const STEPS: Step[] = [
  {
    title: "Customer locks USDC",
    says: "You lock USDC in the source lock on Base. It opens only with a secret your browser keeps.",
    nodes: ["customer", "source"],
    flows: [{ wire: "customer-source" }],
  },
  {
    title: "Aqua releases USDT",
    says: "The provider locks the same value in USDT on Whitechain, from its 1inch Aqua liquidity. Same secret, same rule.",
    nodes: ["provider", "aqua", "destination"],
    flows: [{ wire: "aqua-destination" }],
  },
  {
    title: "Merchant paid",
    says: "Your browser checks that lock, then reveals the secret. The destination lock pays the merchant.",
    nodes: ["destination", "merchant"],
    flows: [{ wire: "destination-merchant" }],
  },
  {
    title: "Provider takes USDC",
    says: "The secret is now public on Whitechain. The provider uses it to claim your USDC on Base.",
    nodes: ["source", "provider"],
    flows: [{ wire: "source-provider" }],
  },
  {
    title: "Refunds",
    says: "If the secret is never revealed, both locks expire. Each side takes its own money back.",
    nodes: ["customer", "source", "destination", "provider"],
    flows: [{ wire: "customer-source", back: true }, { wire: "provider-destination", back: true }],
    fallback: true,
  },
];

/** How long each step shows before the next. */
const STEP_MS = 4200;

export function TrustlessFlow() {
  const [index, setIndex] = useState(0);
  const [held, setHeld] = useState<number | null>(null);
  const [still, setStill] = useState(false);
  const shown = held ?? index;
  const step = STEPS[shown];

  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setStill(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);

  // Play the steps one by one, unless one is held or motion is reduced.
  useEffect(() => {
    if (held !== null || still) return;
    const timer = window.setTimeout(() => setIndex((i) => (i + 1) % STEPS.length), STEP_MS);
    return () => window.clearTimeout(timer);
  }, [index, held, still]);

  const hold = (i: number) => setHeld(i);
  const release = () => {
    if (held === null) return;
    setIndex(held);
    setHeld(null);
  };

  return (
    <div className={styles.trustGrid}>
      <ol className={styles.trustSteps} onMouseLeave={release}>
        {STEPS.map((item, i) => (
          <li key={item.title} className={item.fallback ? styles.trustFallback : undefined}>
            {item.fallback && <span className={styles.trustDivider}>If time runs out</span>}
            <button
              type="button"
              aria-pressed={i === shown}
              className={[styles.trustStep, i === shown ? styles.trustStepOn : ""].join(" ")}
              // Pointer movement, not mouseenter: a row resizing under a still mouse must not
              // switch the step.
              onPointerMove={() => held !== i && hold(i)}
              onFocus={() => hold(i)}
              onBlur={release}
              onClick={() => {
                setIndex(i);
                setHeld(null);
              }}
            >
              <span className={styles.trustNum}>{String(i + 1).padStart(2, "0")}</span>
              <span className={styles.trustText}>
                <span className={styles.trustTitle}>{item.title}</span>
                <span className={styles.trustSays}>
                  <span>{item.says}</span>
                </span>
              </span>
              {i === shown && (
                <span className={styles.trustBar} aria-hidden="true">
                  <i
                    key={`${index}-${held ?? "play"}`}
                    className={held !== null || still ? styles.trustBarFull : styles.trustBarFill}
                    style={{ "--step-ms": `${STEP_MS}ms` } as CSSProperties}
                  />
                </span>
              )}
            </button>
          </li>
        ))}
      </ol>

      <Diagram step={step} />
    </div>
  );
}

function Diagram({ step }: { step: Step }) {
  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const flowOf = (wire: WireKey) => step.flows.find((flow) => flow.wire === wire);

  return (
    <div ref={box} className={styles.trustDiagram} aria-hidden="true">
      {size && (
        <svg className={styles.trustWires} width={size.w} height={size.h}>
          {(Object.keys(WIRES) as WireKey[]).map((key) => {
            const d = wirePath(key, size.w, size.h);
            const flow = flowOf(key);
            return (
              <g
                key={key}
                className={[flow ? styles.trustWireOn : "", flow?.back ? styles.trustWireBack : ""].join(" ")}
              >
                <path d={d} className={styles.trustWireBase} />
                <path d={d} className={styles.trustWireLit} />
                <path
                  // Restart the light when the step changes, so it starts at the right end.
                  key={`${step.title}-${flow?.back ? "back" : "on"}`}
                  d={d}
                  pathLength={100}
                  className={[styles.trustComet, flow?.back ? styles.trustCometBack : ""].join(" ")}
                />
              </g>
            );
          })}
        </svg>
      )}

      {(Object.keys(NODES) as NodeKey[]).map((key) => {
        const node = NODES[key];
        const on = step.nodes.includes(key);
        return (
          <div key={key} className={[styles.trustNodeWrap, on ? styles.trustNodeWrapOn : ""].join(" ")} style={{ left: `${node.x}%`, top: `${node.y}%` }}>
            <div className={styles.trustNode}>
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img src={node.logo} alt="" className={[styles.trustLogo, node.wide ? styles.trustLogoWide : ""].join(" ")} />
            </div>
            <span className={styles.trustLabel}>{node.label}</span>
          </div>
        );
      })}
    </div>
  );
}

/** The wire's SVG path in pixels: straight, or a curve through its bend. */
function wirePath(key: WireKey, w: number, h: number): string {
  const { from, to, bend } = WIRES[key];
  const at = (p: { x: number; y: number }) => `${(p.x / 100) * w} ${(p.y / 100) * h}`;
  const a = NODES[from];
  const b = NODES[to];
  return bend ? `M ${at(a)} Q ${at(bend)} ${at(b)}` : `M ${at(a)} L ${at(b)}`;
}
