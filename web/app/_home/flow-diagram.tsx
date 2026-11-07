"use client";

import { forwardRef, useRef, useState, type ReactNode } from "react";
import { AnimatedBeam } from "@/components/ui/animated-beam";
import styles from "./home.module.css";

type NodeKey = "base" | "eth" | "provider" | "merchant";

const DETAILS: Record<NodeKey, { title: string; text: string }> = {
  base: {
    title: "USDC on Base Sepolia",
    text: "The customer's USDC locks in our contract on Base. It goes to the provider once the secret is out, and comes back to the customer after a timeout.",
  },
  eth: {
    title: "USDC on Ethereum Sepolia",
    text: "The same as Base, on Ethereum Sepolia. A payment can split between the two chains.",
  },
  provider: {
    title: "Provider",
    text: "A liquidity provider. Pays the merchant in USDT on Whitechain straight away, then claims the customer's funds once the secret is out. Fee: 0.3%.",
  },
  merchant: {
    title: "Merchant",
    text: "Receives USDT on Whitechain. Holds no balance on other chains.",
  },
};

type NodeProps = {
  name: NodeKey;
  label: string;
  logo: string;
  badge?: string;
  hovered: NodeKey | null;
  onHover: (name: NodeKey | null) => void;
  children?: ReactNode;
};

/** A circle with a chain logo, and optionally the token's logo overlapping its edge. */
const FlowNode = forwardRef<HTMLDivElement, NodeProps>(({ name, label, logo, badge, hovered, onHover }, ref) => {
  const on = hovered === name;
  return (
    <div className={styles.flowNodeWrap}>
      <div
        ref={ref}
        tabIndex={0}
        className={[styles.flowNode, on ? styles.flowNodeOn : ""].join(" ")}
        onMouseEnter={() => onHover(name)}
        onMouseLeave={() => onHover(null)}
        onFocus={() => onHover(name)}
        onBlur={() => onHover(null)}
      >
        {badge ? (
          // Same pattern as garden-lens' PairIcons: the token in front, the chain behind, overlapping.
          <div className={styles.flowPair}>
            <img src={badge} alt="" className={styles.flowToken} />
            <img src={logo} alt="" className={styles.flowChain} />
          </div>
        ) : (
          <img src={logo} alt="" className={styles.flowLogo} />
        )}
      </div>
      <span className={styles.flowLabel}>{label}</span>
    </div>
  );
});
FlowNode.displayName = "FlowNode";

export function FlowDiagram() {
  const [hovered, setHovered] = useState<NodeKey | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const base = useRef<HTMLDivElement>(null);
  const eth = useRef<HTMLDivElement>(null);
  const provider = useRef<HTMLDivElement>(null);
  const merchant = useRef<HTMLDivElement>(null);

  // Hovering any node stops every ray. The paths of the hovered node light up.
  const paused = hovered !== null;
  const linked = (...ends: NodeKey[]) => hovered !== null && ends.includes(hovered);
  const detail = hovered ? DETAILS[hovered] : null;

  return (
    <div className={styles.flowWrap}>
      <div ref={box} className={styles.flow}>
        <div className={styles.flowColumn}>
          <FlowNode ref={base} name="base" label="Base" logo="/logos/base.png" badge="/logos/usdc.svg" hovered={hovered} onHover={setHovered} />
          <FlowNode ref={eth} name="eth" label="Ethereum" logo="/logos/eth.svg" badge="/logos/usdc.svg" hovered={hovered} onHover={setHovered} />
        </div>

        <div className={styles.flowColumn}>
          <FlowNode ref={provider} name="provider" label="Provider" logo="/logos/uniform-mark.png" hovered={hovered} onHover={setHovered} />
        </div>

        <div className={styles.flowColumn}>
          <FlowNode ref={merchant} name="merchant" label="Whitechain" logo="/logos/whitechain-icon.png" badge="/logos/usdt.svg" hovered={hovered} onHover={setHovered} />
        </div>

        {/* One path and one ray per link. Customer funds flow in blue, USDT flows out in green. */}
        <AnimatedBeam containerRef={box} fromRef={base} toRef={provider} curvature={75} endYOffset={-10} duration={1.8} paused={paused} highlighted={linked("base", "provider")} />
        <AnimatedBeam containerRef={box} fromRef={eth} toRef={provider} curvature={-75} endYOffset={10} duration={1.8} paused={paused} highlighted={linked("eth", "provider")} />
        <AnimatedBeam
          containerRef={box}
          fromRef={provider}
          toRef={merchant}
          duration={1.4}
          gradientStartColor="#26A17B"
          gradientStopColor="#9ff0d2"
          paused={paused} highlighted={linked("provider", "merchant")}
        />
      </div>

      <div className={styles.flowDetail} aria-live="polite">
        {detail ? (
          <>
            <p className={styles.flowDetailTitle}>{detail.title}</p>
            <p className={styles.flowDetailText}>{detail.text}</p>
          </>
        ) : (
          <p className={styles.flowHint}>Hover a node to see what it does.</p>
        )}
      </div>
    </div>
  );
}
