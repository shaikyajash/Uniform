"use client";

// The frame every page sits in: two thin lines down each side and the floating nav.
// The nav links to the Demo Store and the dashboard from every page that has it.
// The nav hides while a Privy window (login, signing) is open over the page.
// Only the home page shows the "How it works" button; the pay page keeps the customer on the payment.
// On the home page it waits until the hero's own big "How it works" button is scrolled away.

import { useModalStatus } from "@privy-io/react-auth";
import { useEffect, useState } from "react";
import Link from "next/link";
import { StarButton } from "./star-button";
import { Wordmark } from "./wordmark";
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
        <nav className={styles.navRight} aria-label="Main">
          {howItWorks && <NavHowItWorks />}
          {NAV_LINKS.map(({ label, href }) => (
            <Link key={href} href={href} className={styles.navLink}>
              {label}
            </Link>
          ))}
        </nav>
        <span className={`${styles.corner} ${styles.cornerTopLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerTopRight}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomLeft}`} aria-hidden="true" />
        <span className={`${styles.corner} ${styles.cornerBottomRight}`} aria-hidden="true" />
      </div>
    </header>
  );
}

const NAV_LINKS = [
  { label: "Dashboard", href: "/dashboard" },
  { label: "Earn", href: "/earn" },
  { label: "Docs", href: "/docs" },
];

/** The element the hero gives its big "How it works" button; the nav's copy shows once it is gone. */
export const HERO_CTA_ID = "hero-cta";

function NavHowItWorks() {
  // Hidden to start, so the opening screen never shows two of the same button.
  const [heroButtonInView, setHeroButtonInView] = useState(true);
  useEffect(() => {
    const heroButton = document.getElementById(HERO_CTA_ID);
    if (!heroButton) return;
    const observer = new IntersectionObserver(([entry]) => setHeroButtonInView(entry.isIntersecting));
    observer.observe(heroButton);
    return () => observer.disconnect();
  }, []);
  return (
    <span className={`${styles.navCta} ${heroButtonInView ? styles.navCtaHidden : ""}`} aria-hidden={heroButtonInView}>
      <StarButton href="#features" size="small">
        HOW IT WORKS
      </StarButton>
    </span>
  );
}

/** The Uniform wordmark (see wordmark.tsx), linking home. */
export function Logo() {
  return (
    <Link href="/" className={styles.logo} aria-label="Uniform home">
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src="/logos/uniform-mark.png" alt="" className={styles.logoMark} />
      <Wordmark className={styles.wordmark} skipFirst />
    </Link>
  );
}
