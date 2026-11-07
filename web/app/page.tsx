import HalftoneWordmarkFooter, { type FooterColumn } from "@/components/ui/halftone-wordmark-footer";
import { Frame, HERO_CTA_ID } from "./_ui/frame";
import { SolidButton, StarButton } from "./_ui/star-button";
import { HeroCanvas } from "./_home/hero-canvas";
import { FlowDiagram } from "./_home/flow-diagram";
import { TrustlessFlow } from "./_home/trustless-flow";
import { EarnSection } from "./_home/earn-section";
import { CustodySection } from "./_home/custody-section";
import styles from "./_home/home.module.css";

/** Partners, Whitechain first. All grey, the same as each other. */
const PARTNERS: { alt: string; src: string; label?: string; height: number }[] = [
  { alt: "Whitechain", src: "/logos/whitechain.svg", height: 26 },
  { alt: "1inch", src: "/logos/1inch.svg", height: 16 },
  { alt: "Uniswap", src: "/logos/uniswap.svg", label: "Uniswap", height: 34 },
];

export default function Home() {
  return (
    <div className={styles.home}>
      <Frame howItWorks />
      <main>
        <Hero />
        <Hatch />
        <Features />
        <Hatch />
        <Trustless />
        <Hatch />
        <EarnSection />
        <Hatch />
        <CustodySection />
        <Hatch />
      </main>
      <Footer />
    </div>
  );
}

function Hero() {
  return (
    <section id="top" className={styles.hero}>
      <HeroCanvas />
      <div className={styles.heroCopy}>
        <h1 className={styles.heroTitle}>
          Why fragment liquidity?
          <span className={styles.fade}>Meet Uniform.</span>
        </h1>
        <p className={styles.heroLede}>
          <span className={styles.line}>Let your users pay with what they want and get paid in what you want.</span>{" "}
          <span className={styles.line}>Powered by trustless atomic swaps. You Trust no one.</span>
        </p>
        <div id={HERO_CTA_ID} className={styles.heroActions}>
          <SolidButton href="/demo">Try the demo store</SolidButton>
          <StarButton href="#features" size="large">
            HOW IT WORKS
          </StarButton>
        </div>
      </div>
      <div className={styles.partners} aria-label="Partners">
        {PARTNERS.map((partner) => (
          <span key={partner.alt} className={styles.partner}>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src={partner.src} alt={partner.alt} className={styles.partnerLogo} style={{ height: partner.height }} />
            {partner.label ? <span className={styles.partnerName}>{partner.label}</span> : null}
          </span>
        ))}
      </div>
    </section>
  );
}

/** The diagonal-striped band between sections. */
function Hatch() {
  return <div className={styles.hatch} aria-hidden="true" />;
}

function Trustless() {
  return (
    <section id="trustless" className={styles.features}>
      <div className={styles.sectionHead}>
        <h2 className={styles.sectionTitle}>
          How we are trustless.
          <span className={styles.dim}>Every lock waits for one secret.</span>
        </h2>
        <p className={styles.sectionLede}>
          No one moves your funds without the secret.
        </p>
      </div>
      <TrustlessFlow />
    </section>
  );
}

function Features() {
  return (
    <section id="features" className={`${styles.features} pb-0!`}>
      <div className={styles.sectionHead}>
        <h2 className={styles.sectionTitle}>
          How it works.
          <span className={styles.dim}>No liquidity split across chains.</span>
        </h2>
        <p className={styles.sectionLede}>
          Merchants get USDT on Whitechain. Customers pay with what they already hold.
        </p>
      </div>
      <FlowDiagram />
    </section>
  );
}

/* ---------- Footer ---------- */

/** Only real places: this site's pages and the chains' block explorers. */
const FOOTER_COLUMNS: FooterColumn[] = [
  {
    title: "Product",
    links: [
      { label: "How it works", href: "#features" },
      { label: "Demo store", href: "/demo" },
      { label: "Dashboard", href: "/dashboard" },
      { label: "Earn as an LP", href: "/earn" },
      { label: "Developer docs", href: "/docs" },
    ],
  },
  {
    title: "Chains",
    links: [
      { label: "Whitechain", href: "https://explorer.testnet.whitechain.io" },
      { label: "Base Sepolia", href: "https://sepolia.basescan.org" },
      { label: "Ethereum Sepolia", href: "https://sepolia.etherscan.io" },
    ],
  },
];

/** The word "uniform" printed in halftone dots, cropped by the page's bottom edge, between the side
 * lines. The component's own type; dots pushed by the pointer turn the blue of the hero's rays. */
function Footer() {
  return (
    <div className={styles.footerFrame}>
      <HalftoneWordmarkFooter
        wordmark="uniform"
        label="Uniform"
        meta="Stablecoin checkout for Whitechain."
        footnote="© 2026 · Built at ETHGlobal Mumbai"
        columns={FOOTER_COLUMNS}
        fill={0.94}
        palette={{ background: "#050506", ink: "#f2f2f3", warm: "#5a8cff" }}
        height="480px"
      />
    </div>
  );
}
