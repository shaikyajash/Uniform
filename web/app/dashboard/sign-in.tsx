"use client";

// Signing in with the merchant's payout wallet: connect it, sign one free message, done.

import { useConnectWallet, usePrivy, useWallets } from "@privy-io/react-auth";
import { useState } from "react";
import { ArrowRight, Loader2, Wallet } from "lucide-react";
import { Button } from "@/components/ui/button";
import { shortAddress } from "@/lib/chain";
import { challenge, explainDashboard, verify } from "@/lib/dashboard";
import { Eyebrow } from "./bits";

export function SignIn() {
  const { ready, login, authenticated } = usePrivy();
  const { wallets } = useWallets();
  const { connectWallet } = useConnectWallet();
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  async function signInWith(address: string) {
    const wallet = wallets.find((w) => w.address === address);
    if (!wallet) return;
    setBusy(address);
    setProblem(null);
    try {
      const { nonce, message } = await challenge(wallet.address);
      const signature = await wallet.sign(message);
      await verify(nonce, signature);
    } catch (error) {
      setProblem(explainDashboard(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="mx-auto grid w-full max-w-[440px] gap-8">
      <div className="grid gap-3">
        <Eyebrow>Merchant dashboard</Eyebrow>
        <h1 className="font-display text-[40px] leading-[1.05] tracking-[-0.03em]">
          Sign in with your
          <br />
          <span className="text-muted-foreground">payout wallet</span>
        </h1>
        <p className="text-[14.5px] leading-[1.7] text-muted-foreground">
          The wallet you are paid to is your login. You sign one message. It is free and moves no money.
        </p>
      </div>

      <div className="overflow-hidden rounded-2xl border border-hair bg-gradient-to-b from-panel-2 to-panel shadow-[inset_0_1px_0_rgba(255,255,255,0.08)]">
        {wallets.length > 0 && (
          <ul className="divide-y divide-hair">
            {wallets.map((wallet) => (
              <li key={wallet.address}>
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => void signInWith(wallet.address)}
                  className="group flex w-full items-center gap-3 px-5 py-4 text-left transition-colors hover:bg-white/[0.03] disabled:opacity-60"
                >
                  <span className="grid size-8 place-items-center rounded-full border border-hair-strong text-muted-foreground">
                    <Wallet className="size-3.5" />
                  </span>
                  <span className="grid flex-1 gap-0.5">
                    <span className="font-mono text-[13.5px] text-foreground">{shortAddress(wallet.address)}</span>
                    <span className="text-[12px] text-faint">
                      {wallet.walletClientType === "privy" ? "Email wallet" : wallet.walletClientType}
                    </span>
                  </span>
                  {busy === wallet.address ? (
                    <Loader2 className="size-4 animate-spin text-muted-foreground" />
                  ) : (
                    <span className="flex items-center gap-1.5 font-mono text-[11px] tracking-[0.08em] text-label uppercase opacity-70 transition-opacity group-hover:opacity-100">
                      Sign in <ArrowRight className="size-3.5" />
                    </span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className={wallets.length > 0 ? "flex gap-2 border-t border-hair p-3" : "grid gap-2 p-3"}>
          <Button
            className="h-11 flex-1 rounded-full bg-white font-mono text-[11.5px] tracking-[0.08em] text-black uppercase hover:bg-white/90"
            disabled={!ready || busy !== null}
            onClick={() => connectWallet()}
          >
            {wallets.length > 0 ? "Another wallet" : "Connect wallet"}
          </Button>
          {!authenticated && (
            <Button
              variant="outline"
              className="h-11 flex-1 rounded-full border-hair-strong bg-transparent font-mono text-[11.5px] tracking-[0.08em] uppercase"
              disabled={!ready || busy !== null}
              onClick={() => login()}
            >
              Email wallet
            </Button>
          )}
        </div>
      </div>

      {problem && (
        <p role="alert" className="border-l-2 border-bad pl-3 text-[13.5px] leading-[1.6] text-label">
          {problem}
        </p>
      )}
    </div>
  );
}
