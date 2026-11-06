"use client";

// The frame every page sits in: two thin lines down each side and the floating nav.
// The nav hides while a Privy window (login, signing) is open over the page.
// Only the home page shows the "How it works" button; the pay page keeps the customer on the payment.

import { useModalStatus } from "@privy-io/react-auth";
import Link from "next/link";
import { StarButton } from "./star-button";
import styles from "./ui.module.css";

export function Frame({ howItWorks = false }: { howItWorks?: boolean }) {
  return (
    <>
      <Guides />
      <SiteHeader howItWorks={howItWorks} />
    </>
  );
}

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

function SiteHeader({ howItWorks }: { howItWorks: boolean }) {
  const { isOpen } = useModalStatus();
  return (
    <header className={`${styles.header} ${isOpen ? styles.headerHidden : ""}`}>
      <div className={styles.headerBox}>
        <Logo />
        {howItWorks && (
          <StarButton href="#features" size="small">
            HOW IT WORKS
          </StarButton>
        )}
        <span className={`${styles.corner} ${styles.cornerTopLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerTopRight}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomRight}`} aria-hidden="true" />
      </div>
    </header>
  );
}

export function Logo() {
  return (
    <Link href="/" className={styles.logo} aria-label="Uniform home">
      <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
        <path fill="currentColor" d="M3 3h5v11a4 4 0 0 0 8 0V3h5v11a9 9 0 0 1-18 0z" />
      </svg>
      UNIFORM
    </Link>
  );
}
