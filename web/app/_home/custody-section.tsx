// The question every reader of the earn section asks: why would anyone hand their money to us?
// Told as a short exchange (the question, the concession, the turn), then the difference between
// depositing into a pool and lending through 1inch Aqua, side by side.
//
// Every row is true of Uniform today: the provider's USDT stays in their wallet, the Payout Pool
// draws it only to pay a merchant for a customer payment that is already locked, and one
// transaction (dock) takes the permission back.

import home from "./home.module.css";
import styles from "./custody-section.module.css";

const ROWS: { topic: string; pool: string; aqua: string }[] = [
  { topic: "Where the money sits", pool: "In the pool's contract", aqua: "In your own wallet" },
  { topic: "What you sign", pool: "A deposit", aqua: "A spending limit" },
  { topic: "Use it elsewhere meanwhile", pool: "Not while it's deposited", aqua: "Yes. It never left" },
  {
    topic: "When it moves",
    pool: "Whenever the pool decides",
    aqua: "Only to pay a merchant, after the customer's payment is locked",
  },
  { topic: "Getting out", pool: "Withdraw, sometimes wait", aqua: "Take the limit back in one transaction" },
];

export function CustodySection() {
  return (
    <section id="custody" className={home.features}>
      <p className={styles.eyebrow}>The obvious question</p>

      <blockquote className={styles.question}>
        <p>&ldquo;Why would anyone hand their on&#8209;chain balance to you?&rdquo;</p>
      </blockquote>

      <div className={styles.reply}>
        <p className={styles.answer}>They wouldn&apos;t.</p>
        <p className={styles.concede}>
          You&apos;re right. Nobody should park their money in a stranger&apos;s pool, ours included.
        </p>
      </div>

      <div className={styles.turn}>
        <div className={styles.turnCopy}>
          <h3 className={styles.turnTitle}>So the money never leaves.</h3>
          <p className={styles.turnText}>
            Uniform is built on 1inch Aqua. A provider deposits nothing. They set a spending limit that lets
            Uniform&apos;s payout contract draw USDT from their wallet, one paid order at a time.
          </p>
          <p className={styles.turnText}>
            Until then it&apos;s their balance, in their wallet, free to use. You&apos;re a provider and a holder at
            the same time.
          </p>
        </div>

        <figure className={styles.compare} aria-label="A pool deposit compared with lending through Aqua">
          <div className={`${styles.cRow} ${styles.cHead}`}>
            <span />
            <span>A pool</span>
            <span className={styles.ours}>Uniform on Aqua</span>
          </div>
          {ROWS.map((row) => (
            <div key={row.topic} className={styles.cRow}>
              <span className={styles.topic}>{row.topic}</span>
              <span className={styles.pool}>{row.pool}</span>
              <span className={styles.ours}>{row.aqua}</span>
            </div>
          ))}
          <figcaption className={styles.caption}>
            Spend the USDT and Uniform simply can&apos;t draw it. Nothing is owed.
          </figcaption>
        </figure>
      </div>
    </section>
  );
}
