"use client";

// A dark pill button with a soft light that keeps running around its edge.
// The light follows a path the size of the button, so the button measures itself.

import { useEffect, useRef, useState, type ReactNode } from "react";
import styles from "./home.module.css";

export function StarButton({ href, size, children }: { href: string; size: "small" | "large"; children: ReactNode }) {
  const ref = useRef<HTMLAnchorElement>(null);
  const [path, setPath] = useState<string>();

  useEffect(() => {
    const button = ref.current;
    if (!button) return;
    const observer = new ResizeObserver(() => {
      const { offsetWidth: w, offsetHeight: h } = button;
      setPath(`path('M 0 0 H ${w} V ${h} H 0 V 0')`);
    });
    observer.observe(button);
    return () => observer.disconnect();
  }, []);

  return (
    <a
      ref={ref}
      href={href}
      className={`${styles.starButton} ${size === "small" ? styles.starSmall : styles.starLarge}`}
      style={path ? ({ "--path": path } as React.CSSProperties) : undefined}
    >
      <span className={styles.starLight} aria-hidden="true" />
      <span className={styles.starFace} aria-hidden="true" />
      <span className={styles.starLabel}>
        {children}
        <DoubleChevron />
      </span>
    </a>
  );
}

function DoubleChevron() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true">
      <path d="M12.5 18s6-4.42 6-6-6-6-6-6" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M5.5 18s6-4.42 6-6-6-6-6-6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
