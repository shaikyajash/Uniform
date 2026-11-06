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
      const content = Math.ceil(main.getBoundingClientRect().height);
      const height = isOpen ? Math.max(content, PRIVY_WINDOW_HEIGHT) : content;
      window.parent.postMessage({ type: "uniform:size", height }, "*");
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(main);
    return () => observer.disconnect();
  }, [isOpen]);

  return null;
}
