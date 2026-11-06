"use client";

// The merchant dashboard: sign in with the payout wallet, then payments, webhooks and settings.

import { useEffect, useState } from "react";
import { LogOut } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { shortAddress } from "@/lib/chain";
import { getShop, signOut, takeWelcome, useSession, type Session, type Settings as ShopSettings } from "@/lib/dashboard";
import { Eyebrow } from "./bits";
import { Payments } from "./payments";
import { Settings } from "./settings";
import { SignIn } from "./sign-in";
import { Webhooks } from "./webhooks";
import { Welcome } from "./welcome";

export function Dashboard() {
  const session = useSession();
  // undefined: on the server, before the browser's session is known.
  if (session === undefined) return null;
  return session ? <SignedIn key={session.token} session={session} /> : <SignIn />;
}

type Tab = "payments" | "webhooks" | "settings";

function SignedIn({ session }: { session: Session }) {
  // Just signed up: open on Settings, where the setup code is, with the new secrets on top.
  const [created] = useState(takeWelcome);
  const [tab, setTab] = useState<Tab>(created ? "settings" : "payments");
  const [settings, setSettings] = useState<ShopSettings | null>(null);

  // Fresh on every tab change: a secret replaced in another tab shows here too.
  useEffect(() => {
    let stop = false;
    getShop(session.token).then(
      (next) => !stop && setSettings(next),
      () => undefined,
    );
    return () => {
      stop = true;
    };
  }, [session.token, tab]);

  return (
    <div className="mx-auto grid w-full max-w-[1080px] gap-8">
      {created && <Welcome created={created} name={session.merchant.name} />}
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div className="grid gap-2">
          <Eyebrow>Merchant dashboard</Eyebrow>
          <h1 className="font-display text-[40px] leading-none tracking-[-0.03em]">{session.merchant.name}</h1>
          <p className="flex items-center gap-2 text-[13px] text-muted-foreground">
            <span className="size-1.5 rounded-full bg-ok" aria-hidden="true" />
            Paid in USDT to <span className="font-mono">{shortAddress(session.merchant.payoutAddress)}</span>
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="font-mono text-[10.5px] tracking-[0.08em] text-muted-foreground uppercase hover:text-foreground"
          onClick={() => void signOut(session.token)}
        >
          <LogOut /> Sign out
        </Button>
      </header>

      <Tabs value={tab} onValueChange={(value) => setTab(value as Tab)} className="gap-6">
        <TabsList className="h-10 rounded-full border border-hair bg-transparent p-1">
          {(["payments", "webhooks", "settings"] as const).map((value) => (
            <TabsTrigger
              key={value}
              value={value}
              className="rounded-full px-4 text-[13px] capitalize text-muted-foreground data-[state=active]:bg-white/[0.08] data-[state=active]:text-foreground"
            >
              {value}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="payments">
          <Payments session={session} />
        </TabsContent>
        <TabsContent value="webhooks">
          <Webhooks session={session} webhookUrl={settings?.webhookUrl ?? null} onSetUp={() => setTab("settings")} />
        </TabsContent>
        <TabsContent value="settings">
          <Settings session={session} settings={settings} onChange={setSettings} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
