"use client";

import { PrivyProvider } from "@privy-io/react-auth";
import type { ReactNode } from "react";
import { customerViemChains, shopViemChain } from "@/lib/chain";

/** Privy gives each customer a wallet from an email login; nothing to install. */
export function Providers({ children }: { children: ReactNode }) {
  const appId = process.env.NEXT_PUBLIC_PRIVY_APP_ID;
  if (!appId) {
    return <p style={{ padding: 24 }}>Set NEXT_PUBLIC_PRIVY_APP_ID in the repo&apos;s .env and restart.</p>;
  }
  return (
    <PrivyProvider
      appId={appId}
      config={{
        loginMethods: ["email"],
        appearance: { theme: "dark", accentColor: "#fafafa" },
        embeddedWallets: { ethereum: { createOnLogin: "users-without-wallets" } },
        defaultChain: shopViemChain,
        // Customer chains too: a cross-chain note is signed for the chain the USDC is on.
        supportedChains: [shopViemChain, ...customerViemChains],
      }}
    >
      {children}
    </PrivyProvider>
  );
}
