"use client";

// The demo store: a small coffee roaster's site that takes payments with Uniform. Styled as its own
// brand on purpose, so the checkout reads as something the merchant plugged in. Uniform shows up
// only where it would on a real store: the checkout button and the "we accept" row.

import { useEffect, useState } from "react";
import { ArrowRight, ArrowUpRight, Check, Minus, Plus, ShoppingBag, X } from "lucide-react";
import Link from "next/link";
import { Bag } from "./bag";
import { CATALOG, STORE, formatUnits6, totalUnits, type Cart, type Product, type Roast } from "./catalog";
import styles from "./demo.module.css";

declare global {
  interface Window {
    Uniform?: { checkout: (target: string | (() => Promise<string>)) => Promise<{ orderId: string }> };
  }
}

type Paid = { orderId: string; reference: string };
type Delivery = { type: string; receivedAt: number } | null;

/** Paper behind the whole document while the store is showing, so overscroll never shows the app's
 * dark background. Set from an effect, not a `:has(.demo)` rule: Next keeps a page you leave in the
 * DOM, hidden, so such a rule would still paint the home page. A hidden page's effects are cleaned
 * up, so this one is undone on the way out. */
function usePaperDocument() {
  useEffect(() => {
    const html = document.documentElement;
    const previous = { html: html.style.background, body: document.body.style.background, scroll: html.style.scrollBehavior };
    html.style.background = "#f4efe6";
    document.body.style.background = "#f4efe6";
    html.style.scrollBehavior = "smooth";
    return () => {
      html.style.background = previous.html;
      document.body.style.background = previous.body;
      html.style.scrollBehavior = previous.scroll;
    };
  }, []);
}

export function Store() {
  usePaperDocument();
  const [cart, setCart] = useState<Cart>({});
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [paid, setPaid] = useState<Paid | null>(null);
  const count = Object.values(cart).reduce((sum, n) => sum + n, 0);

  function change(id: string, by: number) {
    setCart((current) => ({ ...current, [id]: Math.max(0, Math.min(9, (current[id] ?? 0) + by)) }));
  }

  function add(id: string) {
    change(id, 1);
    setPaid(null);
    setOpen(true);
  }

  function checkout() {
    if (!window.Uniform) {
      setProblem("Checkout is still loading. Try again in a second.");
      return;
    }
    setProblem(null);
    setBusy(true);
    let reference = "";
    // Called straight from the click, so the browser allows the payment window.
    window.Uniform.checkout(async () => {
      const response = await fetch("/demo/api/checkout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cart }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? "Couldn't start checkout.");
      reference = body.reference;
      return body.payUrl as string;
    }).then(
      ({ orderId }) => {
        setPaid({ orderId, reference });
        setCart({});
        setBusy(false);
        setOpen(true);
      },
      (error: Error) => {
        setBusy(false);
        if (error.message !== "closed") setProblem(error.message);
      },
    );
  }

  return (
    <div className={styles.demo}>
      <p className={styles.notice}>
        Demo store on testnet. Nothing ships.
        <Link href="/" className={styles.noticeLink}>
          Back to Uniform <ArrowRight className="size-3" />
        </Link>
      </p>

      <header className={styles.header}>
        <div className={styles.headerInner}>
          <a href="#top" className={styles.brand}>
            {STORE.name}
          </a>
          <nav className={styles.nav}>
            <a href="#shop">Coffee</a>
            <a href="#story">Our story</a>
            <a href="#visit">Visit</a>
          </nav>
          <button type="button" className={styles.bagButton} onClick={() => setOpen(true)} aria-label={`Bag, ${count} items`}>
            <ShoppingBag className="size-[18px]" strokeWidth={1.6} />
            {count > 0 && <span className={styles.bagCount}>{count}</span>}
          </button>
        </div>
      </header>

      <main id="top">
        <Hero />
        <Promises />
        <Shop cart={cart} onAdd={add} />
        <Story />
      </main>

      <Footer />

      <Drawer open={open} onClose={() => setOpen(false)}>
        {paid ? (
          <Confirmation paid={paid} onDone={() => { setPaid(null); setOpen(false); }} />
        ) : (
          <BagContents cart={cart} busy={busy} problem={problem} onChange={change} onCheckout={checkout} onClose={() => setOpen(false)} />
        )}
      </Drawer>
    </div>
  );
}

/* ---------- Page ---------- */

function Hero() {
  const [araku, chik, malabar] = [CATALOG[0], CATALOG[1], CATALOG[3]];
  return (
    <section className={styles.hero}>
      <div className={styles.heroCopy}>
        <p className={styles.kicker}>Roasted in Bombay</p>
        <h1 className={styles.heroTitle}>Coffee from the Western Ghats.</h1>
        <p className={styles.heroLede}>Single estates from Karnataka and Andhra. Roasted every Monday, at your door by Wednesday.</p>
        <div className={styles.heroActions}>
          <a href="#shop" className={styles.primary}>
            Shop coffee
          </a>
          <a href="#story" className={styles.textLink}>
            Our story
          </a>
        </div>
      </div>
      <div className={styles.heroArt} aria-hidden="true">
        <div className={styles.sun} />
        <Bag product={chik} className={`${styles.heroBag} ${styles.heroBagLeft}`} />
        <Bag product={malabar} className={`${styles.heroBag} ${styles.heroBagRight}`} />
        <Bag product={araku} className={`${styles.heroBag} ${styles.heroBagFront}`} />
      </div>
    </section>
  );
}

function Promises() {
  const items = [
    { title: "Roasted to order", text: "Every Monday, in small batches." },
    { title: "Free shipping", text: "Anywhere in India, in two days." },
    { title: "Pay with stablecoins", text: "USDC or USDT, from Base or Ethereum." },
  ];
  return (
    <ul className={styles.promises}>
      {items.map((item) => (
        <li key={item.title}>
          <p className={styles.promiseTitle}>{item.title}</p>
          <p className={styles.promiseText}>{item.text}</p>
        </li>
      ))}
    </ul>
  );
}

function Shop({ cart, onAdd }: { cart: Cart; onAdd: (id: string) => void }) {
  return (
    <section id="shop" className={styles.section}>
      <div className={styles.sectionHead}>
        <h2 className={styles.sectionTitle}>This week&apos;s coffee</h2>
        <p className={styles.sectionMeta}>{CATALOG.length} coffees · 250 g bags · whole bean</p>
      </div>
      <ul className={styles.grid}>
        {CATALOG.map((product) => (
          <ProductCard key={product.id} product={product} inBag={cart[product.id] ?? 0} onAdd={() => onAdd(product.id)} />
        ))}
      </ul>
    </section>
  );
}

function ProductCard({ product, inBag, onAdd }: { product: Product; inBag: number; onAdd: () => void }) {
  return (
    <li className={styles.card}>
      <div className={styles.cardArt} style={{ background: product.tint }}>
        <Bag product={product} className={styles.cardBag} />
        <span className={styles.roastTag}>
          <RoastDots roast={product.roast} /> {product.roast}
        </span>
      </div>
      <div className={styles.cardBody}>
        <div className={styles.cardRow}>
          <p className={styles.cardName}>{product.name}</p>
          <p className={styles.cardPrice}>{product.price} USDT</p>
        </div>
        <p className={styles.cardRegion}>{product.region}</p>
        <p className={styles.cardNotes}>{product.notes.join(" · ")}</p>
        <button type="button" className={styles.addButton} onClick={onAdd}>
          {inBag > 0 ? (
            <>
              <Check className="size-3.5" /> In bag · add another
            </>
          ) : (
            "Add to bag"
          )}
        </button>
      </div>
    </li>
  );
}

function RoastDots({ roast }: { roast: Roast }) {
  const level = roast === "Light" ? 1 : roast === "Medium" ? 2 : 3;
  return (
    <span className={styles.roastDots}>
      {[1, 2, 3].map((n) => (
        <i key={n} className={n <= level ? styles.dotOn : undefined} />
      ))}
    </span>
  );
}

function Story() {
  return (
    <section id="story" className={styles.story}>
      <p className={styles.kicker}>The monsoon process</p>
      <p className={styles.storyText}>
        Every June, green coffee sits in open warehouses on the Malabar coast while the monsoon winds blow through. The beans swell,
        turn gold, and lose their sharpness. We named the roastery after that smell.
      </p>
    </section>
  );
}

function Footer() {
  return (
    <footer id="visit" className={styles.footer}>
      <div className={styles.footerTop}>
        <div>
          <p className={styles.footerBrand}>{STORE.name}</p>
          <p className={styles.footerText}>{STORE.full}</p>
        </div>
        <div>
          <p className={styles.footerHead}>Visit</p>
          <p className={styles.footerText}>Pali Hill, Bandra West</p>
          <p className={styles.footerText}>Mumbai 400050</p>
        </div>
        <div>
          <p className={styles.footerHead}>Hours</p>
          <p className={styles.footerText}>Tue to Sun, 8 am to 7 pm</p>
          <p className={styles.footerText}>Roasting on Mondays</p>
        </div>
      </div>
      <div className={styles.footerBottom}>
        <span>© 2026 {STORE.full}</span>
        <Accepts />
      </div>
    </footer>
  );
}

const TOKENS = [
  { src: "/logos/usdc.svg", alt: "USDC" },
  { src: "/logos/usdt.svg", alt: "USDT" },
  { src: "/logos/base.png", alt: "Base" },
  { src: "/logos/eth.svg", alt: "Ethereum" },
];

function Accepts() {
  return (
    <span className={styles.accepts}>
      We accept
      {TOKENS.map((token) => (
        // eslint-disable-next-line @next/next/no-img-element
        <img key={token.alt} src={token.src} alt={token.alt} title={token.alt} className={styles.acceptLogo} />
      ))}
    </span>
  );
}

/** "Payments by Uniform", the way stores credit their payment provider. */
function PoweredBy() {
  return (
    <a href="/" target="_blank" rel="noreferrer" className={styles.poweredBy}>
      Payments by
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src="/logos/uniform-mark.png" alt="" className={styles.poweredMark} />
      Uniform
    </a>
  );
}

/* ---------- Bag drawer ---------- */

function Drawer({ open, onClose, children }: { open: boolean; onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  return (
    <div className={[styles.drawerRoot, open ? styles.drawerOpen : ""].join(" ")} aria-hidden={!open}>
      <div className={styles.scrim} onClick={onClose} />
      <aside className={styles.drawer} role="dialog" aria-label="Your bag" inert={!open}>
        {children}
      </aside>
    </div>
  );
}

function BagContents({
  cart,
  busy,
  problem,
  onChange,
  onCheckout,
  onClose,
}: {
  cart: Cart;
  busy: boolean;
  problem: string | null;
  onChange: (id: string, by: number) => void;
  onCheckout: () => void;
  onClose: () => void;
}) {
  const lines = CATALOG.filter((product) => (cart[product.id] ?? 0) > 0);
  const total = formatUnits6(totalUnits(cart));

  return (
    <>
      <div className={styles.drawerHead}>
        <p>Your bag</p>
        <button type="button" onClick={onClose} className={styles.iconButton} aria-label="Close">
          <X className="size-4" />
        </button>
      </div>

      {lines.length === 0 ? (
        <div className={styles.empty}>
          <p>Your bag is empty.</p>
          <a href="#shop" onClick={onClose} className={styles.primary}>
            Shop coffee
          </a>
        </div>
      ) : (
        <>
          <ul className={styles.lines}>
            {lines.map((product) => (
              <li key={product.id} className={styles.line}>
                <div className={styles.lineArt} style={{ background: product.tint }}>
                  <Bag product={product} className={styles.lineBag} />
                </div>
                <div className={styles.lineBody}>
                  <div className={styles.cardRow}>
                    <p className={styles.lineName}>{product.name}</p>
                    <p className={styles.linePrice}>{product.price} USDT</p>
                  </div>
                  <p className={styles.lineMeta}>
                    {product.roast} roast · 250 g
                  </p>
                  <Stepper n={cart[product.id] ?? 0} onLess={() => onChange(product.id, -1)} onMore={() => onChange(product.id, 1)} />
                </div>
              </li>
            ))}
          </ul>

          <div className={styles.summary}>
            <div className={styles.sumRow}>
              <span>Subtotal</span>
              <span>{total} USDT</span>
            </div>
            <div className={styles.sumRow}>
              <span>Shipping</span>
              <span>Free</span>
            </div>
            <div className={`${styles.sumRow} ${styles.sumTotal}`}>
              <span>Total</span>
              <span>{total} USDT</span>
            </div>
            <button type="button" className={styles.checkout} disabled={busy} onClick={onCheckout}>
              {busy ? "Opening checkout…" : "Checkout"}
            </button>
            {problem && <p className={styles.problem}>{problem}</p>}
            <div className={styles.payNote}>
              <Accepts />
              <PoweredBy />
            </div>
          </div>
        </>
      )}
    </>
  );
}

function Stepper({ n, onLess, onMore }: { n: number; onLess: () => void; onMore: () => void }) {
  return (
    <span className={styles.stepper}>
      <button type="button" aria-label="One less" onClick={onLess}>
        <Minus className="size-3" />
      </button>
      <span>{n}</span>
      <button type="button" aria-label="One more" onClick={onMore}>
        <Plus className="size-3" />
      </button>
    </span>
  );
}

/** After paying: the order, and whether the store's server got Uniform's signed webhook. */
function Confirmation({ paid, onDone }: { paid: Paid; onDone: () => void }) {
  const [delivery, setDelivery] = useState<Delivery>(null);
  useEffect(() => {
    if (delivery) return;
    const look = () =>
      fetch(`/demo/api/webhook?orderId=${paid.orderId}`, { cache: "no-store" })
        .then((response) => response.json())
        .then((body: { delivery: Delivery }) => body.delivery && setDelivery(body.delivery))
        .catch(() => {});
    const timer = setInterval(look, 2_000);
    const first = setTimeout(look, 0);
    return () => {
      clearInterval(timer);
      clearTimeout(first);
    };
  }, [paid.orderId, delivery]);

  return (
    <>
      <div className={styles.drawerHead}>
        <p>Order {paid.reference}</p>
        <button type="button" onClick={onDone} className={styles.iconButton} aria-label="Close">
          <X className="size-4" />
        </button>
      </div>
      <div className={styles.thanks}>
        <span className={styles.thanksMark}>
          <Check className="size-5" />
        </span>
        <p className={styles.thanksTitle}>Thank you.</p>
        <p className={styles.thanksText}>Your coffee goes into Monday&apos;s roast.</p>
        <dl className={styles.status}>
          <div>
            <dt>Payment</dt>
            <dd>Received</dd>
          </div>
          <div>
            <dt>Order</dt>
            <dd className={delivery ? undefined : styles.pending}>{delivery ? "Confirmed" : "Confirming…"}</dd>
          </div>
        </dl>
        <div className={styles.thanksActions}>
          <a href={`/receipt/${paid.orderId}`} target="_blank" rel="noreferrer" className={styles.textLink}>
            Receipt <ArrowUpRight className="size-3.5" />
          </a>
          <button type="button" onClick={onDone} className={styles.primary}>
            Continue shopping
          </button>
        </div>
      </div>
    </>
  );
}
