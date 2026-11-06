import { Frame, Logo } from "./_ui/frame";
import { StarButton } from "./_ui/star-button";
import { HeroCanvas } from "./_home/hero-canvas";
import { LiveLog } from "./_home/live-log";
import styles from "./_home/home.module.css";

/** What Uniform runs on today, as plain wordmarks. */
const WORDMARKS = ["USDT", "USDC", "Whitechain", "Base", "1inch Aqua"];

export default function Home() {
  return (
    <div className={styles.home}>
      <Frame howItWorks />
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

function Hero() {
  return (
    <section id="top" className={styles.hero}>
      <HeroCanvas />
      <div className={styles.heroCopy}>
        <h1 className={styles.heroTitle}>
          The checkout for stablecoins
          <span className={styles.fade}>across chains</span>
        </h1>
        <p className={styles.heroLede}>
          <span className={styles.line}>Customers pay with an email login and no gas.</span>{" "}
          <span className={styles.line}>Shops receive USDT on Whitechain.</span>
        </p>
        <div className={styles.heroActions}>
          <StarButton href="#features" size="large">
            HOW IT WORKS
          </StarButton>
        </div>
      </div>
      <div className={styles.marquee} aria-label="Works with">
        <div className={styles.marqueeTrack}>
          {[...WORDMARKS, ...WORDMARKS, ...WORDMARKS, ...WORDMARKS].map((name, i) => (
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
          Customers pay with USDT on Whitechain or USDC on Base. The shop receives USDT on Whitechain, and the customer
          pays no gas.
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
          id="paths"
          title="Pay with what you hold"
          text="USDT on Whitechain directly, or USDC from Base through a liquidity provider. EURC and ETH are next."
        >
          <PathsMock />
        </Block>
        <Block
          id="guardrails"
          title="Built-in guardrails"
          text="Gas budgets, order caps and a dry run before every send keep the Relayer bounded and safe."
        >
          <GuardrailsMock />
        </Block>
        <Block
          id="checkout"
          title="Checkout with an email"
          text="The customer logs in with an email and gets a wallet. They sign once, and Uniform pays the gas."
        >
          <CheckoutMock />
        </Block>
      </div>
    </section>
  );
}

type BlockProps = { id?: string; title: string; text: string; children: React.ReactNode };

function Block({ id, title, text, children }: BlockProps) {
  return (
    <article id={id} className={styles.block}>
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
          <i className={styles.liveDot} /> CROSS-CHAIN PAYMENT · EXAMPLE
        </span>
        <span className={styles.faint}>Base → Whitechain</span>
      </div>
      <div className={styles.flow}>
        <div className={styles.node}>
          <small>CUSTOMER</small>
          <b>Locks USDC</b>
          <em>on base</em>
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
          <em className={styles.hotNote}>provider found</em>
        </div>
        <i className={styles.wire} />
        <div className={styles.outs}>
          <span>
            Shop paid <i className={styles.okDot} />
          </span>
          <span>
            Provider repaid <i className={styles.okDot} />
          </span>
        </div>
      </div>
      <LiveLog
        className={styles.log}
        lines={[
          "Customer locked 12.036 USDC on Base",
          "Provider locked 12 USDT on Whitechain for the shop",
          "Browser checked the provider's lock, secret revealed",
          "Shop claimed 12 USDT, provider claimed 12.036 USDC",
        ]}
      />
      <div className={styles.stats}>
        <div>
          <small>CUSTOMER GAS</small>
          <b>0</b>
        </div>
        <div>
          <small>PROVIDER FEE</small>
          <b>0.3%</b>
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
  { name: "USDT direct", meta: "PATH A · WHITECHAIN", status: "TESTNET", from: 4, to: 38, color: "cyan" },
  { name: "USDC from Base", meta: "PATH B · CROSS-CHAIN", status: "TESTNET", from: 22, to: 68, color: "green" },
  { name: "EURC or ETH", meta: "PATH C · SWAP ON REDEEM", status: "IN PROGRESS", from: 48, to: 92, color: "violet" },
] as const;

function PathsMock() {
  return (
    <div className={styles.panel}>
      <div className={styles.panelBar}>
        <span className={styles.panelTitle}>Payment paths</span>
        <span className={styles.chip}>2 ON TESTNET</span>
      </div>
      <div className={styles.timeline}>
        {PATHS.map((path) => (
          <div key={path.name} className={styles.track}>
            <div className={styles.trackHead}>
              <span>{path.name}</span>
              <span className={styles.faint}>{path.status}</span>
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
        <span>The shop always gets USDT</span>
        <span>0 gas for customers</span>
      </div>
    </div>
  );
}

/* ---------- Block 3: the Relayer's guardrails ---------- */

const RULES = [
  { name: "Daily gas budget", hint: "Per shop, per day", value: "Enforced" },
  { name: "Order cap", hint: "Largest order sent", value: "100 USDT" },
  { name: "Dry run", hint: "Simulate before sending", value: "Always" },
  { name: "Blocked payers", hint: "USDT block list", value: "Refused" },
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
          <span className={styles.select}>{rule.value}</span>
        </div>
      ))}
    </div>
  );
}

/* ---------- Block 4: the pay page, as the customer sees it ---------- */

function CheckoutMock() {
  return (
    <div className={styles.panel}>
      <div className={styles.panelBar}>
        <span className={styles.panelTitle}>Pay page</span>
        <span className={styles.faint}>PREVIEW</span>
      </div>
      <div className={styles.checkout}>
        <small>PAY</small>
        <b className={styles.checkoutShop}>Your shop</b>
        <p className={styles.checkoutAmount}>
          12.00 <span>USDT</span>
        </p>
        <span className={styles.faint}>on Whitechain · no gas needed</span>
        <span className={styles.checkoutButton}>Log in with email to pay</span>
        <span className={styles.faint}>We create a wallet for you. Nothing to install.</span>
      </div>
    </div>
  );
}

/* ---------- Footer ---------- */

function Footer() {
  return (
    <footer className={styles.footer}>
      <div className={styles.footerTop}>
        <div className={styles.footerBrand}>
          <Logo />
          <p>Stablecoin checkout across chains. Shops are paid in USDT on Whitechain, customers pay no gas.</p>
        </div>
      </div>
      <div className={styles.footerBottom}>
        <span>Built at ETHGlobal Mumbai, 2026.</span>
        <span>
          <i className={styles.okDot} /> Running on Whitechain testnet
        </span>
      </div>
    </footer>
  );
}
