"use client";

// Inside the checkout widget's card, the pay page tells the card how tall to be, so the card fits
// its content instead of being a tall empty box. While a Privy window (login, signing) is open it
// asks for more room, because that window is drawn inside the card too.

import { useModalStatus } from "@privy-io/react-auth";
import { useEffect } from "react";

/** Room Privy's login and signing windows need inside the card. */
const PRIVY_WINDOW_HEIGHT = 620;

export function EmbedSize() {
  const { isOpen } = useModalStatus();

  useEffect(() => {
    if (window.parent === window) return;
    const main = document.querySelector("main");
    if (!main) return;
    const report = () => {
      // A few pixels spare: rounding must never leave the last line (or the first) clipped.
      const content = Math.ceil(main.getBoundingClientRect().height) + 6;
      const height = isOpen ? Math.max(content, PRIVY_WINDOW_HEIGHT) : content;
      window.parent.postMessage({ type: "uniform:size", height }, "*");
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(main);
    return () => observer.disconnect();
  }, [isOpen]);

  // Escape pressed inside the card goes to this frame, not the merchant's page: pass it on, so
  // the widget closes the card the same way wherever the focus is.
  useEffect(() => {
    if (window.parent === window) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") window.parent.postMessage({ type: "uniform:close" }, "*");
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  return null;
}
