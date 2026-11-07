"use client";

// Whether this page is shown inside the checkout widget's card (an iframe on a merchant's page).
// layout.tsx marks <html data-embedded> before the first paint; CSS reads it with the `embedded:`
// variant, and components that need a different layout read it here.

import { useSyncExternalStore } from "react";

const subscribe = () => () => undefined;
const read = () => document.documentElement.dataset.embedded !== undefined;

/** False on the server and during hydration; the real value right after. */
export function useEmbedded(): boolean {
  return useSyncExternalStore(subscribe, read, () => false);
}
