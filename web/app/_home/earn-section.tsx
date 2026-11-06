"use client";

// "Let your money earn for you": how a liquidity provider (LP) earns, told as a statement that
// prints itself one line at a time, beside three plain facts and the way in (/earn).
//
// The numbers follow the real rules: the LP pays the merchant in USDT and claims the customer's
// USDC, which is the order plus a 0.3% fee (12 USDT out, 12.036 USDC back). The orders themselves
// are an example, and the statement says so.

import { useEffect, useState } from "react";
import { SolidButton } from "@/app/_ui/star-button";
import home from "./home.module.css";
import styles from "./earn-section.module.css";

type Line =
  | { kind: "head"; label: string; value: string; note?: string }
  | { kind: "out"; label: string; value: string }
  | { kind: "in"; label: string; value: string; fee: number };

/** One order: 12 USDT out on Whitechain, 12.036 USDC back on Base. */
const LINES: Line[] = [
  { kind: "head", label: "In your wallet", value: "1,000.00 USDT" },
  { kind: "head", label: "Offered through 1inch Aqua", value: "1,000.00 USDT", note: "still in your wallet" },
  { kind: "out", label: "Order 7F2 · merchant paid", value: "−12.00 USDT" },
  { kind: "in", label: "Claimed on Base", value: "+12.036 USDC", fee: 0.036 },
  { kind: "out", label: "Order 81B · merchant paid", value: "−40.00 USDT" },
  { kind: "in", label: "Claimed on Ethereum", value: "+40.120 USDC", fee: 0.12 },
  { kind: "out", label: "Order 9C0 · merchant paid", value: "−25.00 USDT" },
  { kind: "in", label: "Claimed on Base", value: "+25.075 USDC", fee: 0.075 },
];

const LINE_MS = 900;
const HOLD_MS = 3200;

const FACTS = [
  { n: "01", title: "You keep custody", text: "Your USDT stays in your wallet. Uniform only draws on it when a payment needs it." },
  { n: "02", title: "Every payment pays you", text: "You pay the merchant in USDT and claim the customer's USDC, plus 0.3%." },
  { n: "03", title: "Leave any time", text: "One transaction takes your offer back. Nothing is locked in a pool." },
];

export function EarnSection() {
  return (
    <section id="earn" className={home.features}>
      <div className={home.sectionHead}>
        <h2 className={home.sectionTitle}>
          Let your money earn for you.
          <span className={home.dim}>Without losing custody.</span>
        </h2>
        <p className={home.sectionLede}>Provide the USDT merchants are paid in. Earn on every payment you fill.</p>
      </div>

      <div className={styles.grid}>
        <Statement />

        <div className={styles.side}>
          <ol className={styles.facts}>
            {FACTS.map((fact) => (
              <li key={fact.n} className={styles.fact}>
                <span className={styles.factN}>{fact.n}</span>
                <div>
                  <p className={styles.factTitle}>{fact.title}</p>
                  <p className={styles.factText}>{fact.text}</p>
                </div>
              </li>
            ))}
          </ol>
          <div className={styles.cta}>
            <SolidButton href="/earn">Become an LP</SolidButton>
          </div>
        </div>
      </div>
    </section>
  );
}

/** A statement that prints a line at a time, then starts over. */
function Statement() {
  const [shown, setShown] = useState(LINES.length);

  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let count = 0;
    let timer = 0;
    const tick = () => {
      count = count >= LINES.length ? 0 : count + 1;
      setShown(count);
      timer = window.setTimeout(tick, count >= LINES.length ? HOLD_MS : LINE_MS);
    };
    tick();
    return () => window.clearTimeout(timer);
  }, []);

  const earned = LINES.slice(0, shown).reduce((sum, line) => sum + (line.kind === "in" ? line.fee : 0), 0);

  return (
    <figure className={styles.statement} aria-label="Example statement for a liquidity provider">
      <header className={styles.stHead}>
        <span>Statement</span>
        <span>Example</span>
      </header>
      <ul className={styles.rows}>
        {LINES.map((line, i) => (
          <li
            key={i}
            className={[styles.row, styles[line.kind], i < shown ? styles.rowOn : "", i === shown - 1 ? styles.rowLast : ""].join(" ")}
          >
            <span className={styles.label}>
              {line.label}
              {line.kind === "head" && line.note && <em className={styles.note}>{line.note}</em>}
            </span>
            <span className={styles.leader} aria-hidden="true" />
            <span className={styles.value}>{line.value}</span>
          </li>
        ))}
      </ul>
      <footer className={styles.total}>
        <span>Earned</span>
        <span className={styles.totalValue}>+{earned.toFixed(3)} USDC</span>
      </footer>
      <figcaption className={styles.feeNote}>0.3% per payment · USDT on Whitechain</figcaption>
    </figure>
  );
}
