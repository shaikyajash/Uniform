import { Geist, Geist_Mono, Google_Sans } from "next/font/google";
import { HeroCanvas } from "./_home/hero-canvas";
import { LiveLog } from "./_home/live-log";
import { StarButton } from "./_home/star-button";
import styles from "./_home/home.module.css";

const geist = Geist({ variable: "--font-geist", subsets: ["latin"] });
const geistMono = Geist_Mono({ variable: "--font-geist-mono", subsets: ["latin"] });
const display = Google_Sans({ variable: "--font-display", subsets: ["latin"], weight: "400" });

const NAV = [
  { label: "About", href: "#about" },
  { label: "Features", href: "#features" },
  { label: "How it works", href: "#features" },
  { label: "Docs", href: "https://github.com/shaikyajash/Uniform" },
];

/** Assets and networks Uniform works with, as plain wordmarks. */
const WORDMARKS = ["USDT", "USDC", "EURC", "ETH", "Whitechain", "Base", "Ethereum", "Uniswap", "1inch Aqua"];

export default function Home() {
  return (
    <div className={`${styles.home} ${geist.variable} ${geistMono.variable} ${display.variable}`}>
      <Guides />
      <Header />
      <main>
        <Hero />
        <Hatch />
        <Features />
        <Hatch />
      </main>
      <Footer />
    </div>
  );
}

/** The frame the whole page sits in: two thin vertical lines down each side. */
function Guides() {
  return (
    <div className={styles.guides} aria-hidden="true">
      <span className={styles.guideOuterLeft} />
      <span className={styles.guideInnerLeft} />
      <span className={styles.guideInnerRight} />
      <span className={styles.guideOuterRight} />
    </div>
  );
}

function Logo() {
  return (
    <a href="#top" className={styles.logo} aria-label="Uniform home">
      <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M3 3h5v11a4 4 0 0 0 8 0V3h5v11a9 9 0 0 1-18 0z" />
      </svg>
      UNIFORM
    </a>
  );
}

function Header() {
  return (
    <header className={styles.header}>
      <div className={styles.headerBox}>
        <Logo />
        <nav className={styles.nav} aria-label="Main">
          {NAV.map((item) => (
            <a key={item.label} href={item.href}>
              {item.label}
              <span className={styles.navUnderline} aria-hidden="true" />
            </a>
          ))}
        </nav>
        <StarButton href="#get-started" size="small">
          GET STARTED
        </StarButton>
        <span className={`${styles.corner} ${styles.cornerTopLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerTopRight}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomRight}`} aria-hidden="true" />
      </div>
    </header>
  );
}

function Hero() {
  return (
    <section id="top" className={styles.hero}>
      <HeroCanvas />
      <div className={styles.heroCopy}>
        <h1 className={styles.heroTitle}>
          The checkout for stablecoins
          <span className={styles.fade}>on every chain</span>
        </h1>
        <p className={styles.heroLede}>
          <span className={styles.line}>Customers pay from any chain with the token they hold.</span>{" "}
          <span className={styles.line}>Shops receive USDT on Whitechain in seconds, gasless.</span>
        </p>
        <div className={styles.heroActions}>
          <StarButton href="#get-started" size="large">
            GET STARTED
          </StarButton>
          <a className={styles.solidButton} href="#demo">
            <span className={styles.solidShine} aria-hidden="true" />
            REQUEST A DEMO
          </a>
        </div>
      </div>
      <div className={styles.marquee} aria-label="Works with">
        <div className={styles.marqueeTrack}>
          {[...WORDMARKS, ...WORDMARKS].map((name, i) => (
            <span key={i} aria-hidden={i >= WORDMARKS.length}>
              {name}
            </span>
          ))}
        </div>
      </div>
    </section>
  );
}

/** The diagonal-striped band between sections. */
function Hatch() {
  return <div className={styles.hatch} aria-hidden="true" />;
}

function Features() {
  return (
    <section id="features" className={styles.features}>
      <div className={styles.sectionHead}>
        <h2 className={styles.sectionTitle}>
          Built for stablecoins.
          <span className={styles.dim}>Settled on Whitechain.</span>
        </h2>
        <p className={styles.sectionLede}>
          Uniform turns any token on any chain into USDT for the shop, with no gas for the customer and refunds that
          find their way back.
        </p>
      </div>

      <div className={styles.grid}>
        <Block
          title="Cross-chain in one tap"
          text="The customer locks on their chain, a liquidity provider pays the shop on Whitechain, and one secret settles both."
        >
          <PipelineMock />
        </Block>
        <Block
          title="Pay with what you hold"
          text="USDT straight, USDC or EURC from Base, or ETH from Sepolia, swapped and settled as USDT."
        >
          <PathsMock />
        </Block>
        <Block
          title="Built-in guardrails"
          text="Gas budgets, order caps and a dry run before every send keep the Relayer bounded and safe."
        >
          <GuardrailsMock />
        </Block>
        <Block title="Shop inbox" text="Payments, refunds, payouts and webhooks for your shop in one focused queue.">
          <InboxMock />
        </Block>
      </div>
    </section>
  );
}

function Block({ title, text, children }: { title: string; text: string; children: React.ReactNode }) {
  return (
    <article className={styles.block}>
      <div className={styles.stage}>{children}</div>
      <h3 className={styles.blockTitle}>{title}</h3>
      <p className={styles.blockText}>{text}</p>
    </article>
  );
}

/* ---------- Block 1: a cross-chain payment, live ---------- */

function PipelineMock() {
  return (
    <div className={styles.panel}>
      <div className={styles.panelBar}>
        <span>
          <i className={styles.liveDot} /> CROSS-CHAIN PAYMENT · LIVE
        </span>
        <span className={styles.faint}>3 chains · 0 failures</span>
      </div>
      <div className={styles.flow}>
        <div className={styles.node}>
          <small>CUSTOMER</small>
          <b>Locks USDC</b>
          <em>base · sepolia</em>
        </div>
        <i className={styles.wire} />
        <div className={styles.node}>
          <small>HASHLOCK</small>
          <b>Secret sealed</b>
          <em>from signature</em>
        </div>
        <i className={styles.wire} />
        <div className={`${styles.node} ${styles.nodeHot}`}>
          <small>MATCHMAKER</small>
          <b>Routing liquidity</b>
          <span className={styles.dots}>
            <i />
            <i />
            <i />
          </span>
          <em className={styles.hotNote}>bob found</em>
        </div>
        <i className={styles.wire} />
        <div className={styles.outs}>
          <span>
            Shop paid <i className={styles.okDot} />
          </span>
          <span>
            Bob repaid <i className={styles.okDot} />
          </span>
          <span>
            Receipt sent <i className={styles.warnDot} />
          </span>
        </div>
      </div>
      <LiveLog
        className={styles.log}
        lines={[
          "Alice locked 12.08 USDC on Base, hashlock 0x9f3c…",
          "Bob locked 12 USDT on Whitechain, same hashlock",
          "Browser checked Bob's lock, secret revealed",
          "Shop claimed 12 USDT, Bob claimed 12.08 USDC",
        ]}
      />
      <div className={styles.stats}>
        <div>
          <small>CUSTOMER GAS</small>
          <b>0</b>
        </div>
        <div>
          <small>SETTLES IN</small>
          <b>seconds</b>
        </div>
        <div>
          <small>SHOP GETS</small>
          <b>USDT</b>
        </div>
        <div className={styles.statRight}>
          <small>NETWORK</small>
          <b className={styles.faint}>Whitechain</b>
        </div>
      </div>
    </div>
  );
}

/* ---------- Block 2: the three ways to pay ---------- */

const PATHS = [
  { name: "USDT direct", meta: "PATH A · WHITECHAIN", from: 4, to: 38, color: "cyan" },
  { name: "USDC or EURC from Base", meta: "PATH B · CROSS-CHAIN", from: 22, to: 68, color: "green" },
  { name: "ETH from Sepolia", meta: "PATH C · SWAP ON REDEEM", from: 48, to: 92, color: "violet" },
] as const;

function PathsMock() {
  return (
    <div className={styles.panel}>
      <div className={styles.panelBar}>
        <span className={styles.panelTitle}>
          Payment paths <span className={styles.faint}>TO SETTLE</span>
        </span>
        <span className={styles.chip}>3 LIVE</span>
      </div>
      <div className={styles.timeline}>
        <div className={styles.ticks}>
          {["0s", "2s", "4s", "6s", "8s", "10s", "12s"].map((tick) => (
            <span key={tick}>{tick}</span>
          ))}
        </div>
        {PATHS.map((path) => (
          <div key={path.name} className={styles.track}>
            <div className={styles.trackHead}>
              <span>{path.name}</span>
              <span className={styles.faint}>LIVE</span>
            </div>
            <small className={styles.faint}>{path.meta}</small>
            <div className={styles.rail}>
              <i
                className={`${styles.bar} ${styles[path.color]}`}
                style={{ left: `${path.from}%`, width: `${path.to - path.from}%` }}
              />
            </div>
          </div>
        ))}
      </div>
      <div className={styles.panelFoot}>
        <span>3 payment paths</span>
        <span>0 gas for customers</span>
      </div>
    </div>
  );
}

/* ---------- Block 3: the Relayer's guardrails ---------- */

const RULES = [
  { name: "Daily gas budget", hint: "Per shop, per chain", value: "Enforce" },
  { name: "Order cap", hint: "Largest order sent", value: "100 USDT" },
  { name: "Dry run", hint: "Simulate before sending", value: "Always" },
  { name: "Blocked payers", hint: "USDT block list", value: "Refuse" },
  { name: "Note expiry", hint: "Signed notes run out", value: "10 min" },
  { name: "Allowed calls", hint: "Typed jobs, no raw calldata", value: "DirectPay" },
];

function GuardrailsMock() {
  return (
    <div className={`${styles.panel} ${styles.rules}`}>
      {RULES.map((rule) => (
        <div key={rule.name} className={styles.rule}>
          <div>
            <b>{rule.name}</b>
            <small>{rule.hint}</small>
          </div>
          <span className={styles.select}>
            {rule.value} <i aria-hidden="true">⌄</i>
          </span>
        </div>
      ))}
      <div className={styles.popover} aria-hidden="true">
        <div className={styles.popoverHead}>Enforce</div>
        <span>Monitor</span>
        <span className={styles.popoverOn}>
          Enforce <i>✓</i>
        </span>
        <span>Off</span>
      </div>
    </div>
  );
}

/* ---------- Block 4: the shop's inbox ---------- */

const INBOX = [
  { glyph: "₮", tone: "green", badge: "ok", title: "Payment received", text: "Order #1001 paid 12 USDT on Whitechain", when: "2m" },
  { glyph: "↺", tone: "amber", badge: "warn", title: "Refund requested", text: "Customer asked for 4 USDT back", when: "1h" },
  { glyph: "$", tone: "blue", badge: "ok", title: "Liquidity repaid", text: "Bob claimed 12.08 USDC on Base", when: "3h" },
  { glyph: "↗", tone: "violet", badge: "ok", title: "Webhook delivered", text: "order.paid sent to kettle.shop", when: "1d" },
] as const;

function InboxMock() {
  return (
    <div className={styles.panel}>
      <div className={styles.panelBar}>
        <span className={styles.panelTitle}>
          Inbox <span className={styles.faint}>···</span>
        </span>
        <span className={styles.faint}>⇅</span>
      </div>
      <ul className={styles.inbox}>
        {INBOX.map((item, i) => (
          <li key={item.title} className={i === 0 ? styles.inboxActive : undefined}>
            <span className={`${styles.avatar} ${styles[item.tone]}`}>
              {item.glyph}
              <i className={item.badge === "ok" ? styles.badgeOk : styles.badgeWarn} />
            </span>
            <span className={styles.inboxText}>
              <b>{item.title}</b>
              <small>{item.text}</small>
            </span>
            <span className={styles.faint}>{item.when}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/* ---------- Footer ---------- */

const FOOTER = [
  { title: "Product", links: ["Pay Button", "Shop Dashboard", "Supported assets", "Refunds"] },
  { title: "Developers", links: ["Documentation", "Order Desk API", "Webhooks", "Contracts"] },
  { title: "Company", links: ["About", "ETHGlobal Mumbai", "Contact"] },
  { title: "Legal", links: ["Privacy", "Terms", "Security"] },
];

function Footer() {
  return (
    <footer className={styles.footer}>
      <div className={styles.footerTop}>
        <div className={styles.footerBrand}>
          <Logo />
          <p>Stablecoin checkout across chains. Shops are paid in USDT on Whitechain, customers pay no gas.</p>
          <div className={styles.socials}>
            <a href="https://github.com/shaikyajash/Uniform">GitHub</a>
            <a href="#x">X</a>
          </div>
        </div>
        {FOOTER.map((column) => (
          <div key={column.title} className={styles.footerColumn}>
            <h4>{column.title}</h4>
            {column.links.map((link) => (
              <a key={link} href="#">
                {link}
                {link === "ETHGlobal Mumbai" && <span className={styles.pill}>2026</span>}
              </a>
            ))}
          </div>
        ))}
      </div>
      <div className={styles.footerBottom}>
        <span>© 2026 Uniform. All rights reserved.</span>
        <span>
          <i className={styles.okDot} /> Running on Whitechain testnet
        </span>
      </div>
    </footer>
  );
}
